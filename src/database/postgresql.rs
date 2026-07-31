use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use regex::Regex;
use tokio_postgres::config::SslMode;
use tokio_postgres::{Client, NoTls};

use crate::core::types::{
    ColumnInfo, DatabaseType, IndexInfo, QueryPlan, QueryPlanNode, SchemaSnapshot, TableSchema,
};
use crate::database::connection::DatabaseConnector;

pub struct PostgresConnector {
    client: Option<Client>,
    simple_mode: bool,
}

impl PostgresConnector {
    pub fn new() -> Self {
        Self { client: None, simple_mode: false }
    }

    fn parse_index_columns(index_def: &str) -> Vec<String> {
        let re = Regex::new(r"\((?P<cols>[^\)]+)\)").expect("valid postgres index regex");
        let cols = re
            .captures(index_def)
            .and_then(|caps| caps.name("cols"))
            .map(|m| m.as_str())
            .unwrap_or_default();

        cols.split(',')
            .map(|c| c.trim().trim_matches('"').to_string())
            .filter(|c| !c.is_empty())
            .collect()
    }

    fn parse_plan_node(value: &serde_json::Value) -> QueryPlanNode {
        let node_type = value
            .get("Node Type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Unknown")
            .to_string();
        let cost = value
            .get("Total Cost")
            .and_then(serde_json::Value::as_f64)
            .or_else(|| {
                value
                    .get("Startup Cost")
                    .and_then(serde_json::Value::as_f64)
            });
        let rows = value.get("Plan Rows").and_then(serde_json::Value::as_f64);
        let index_used = value
            .get("Index Name")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string);

        let children = value
            .get("Plans")
            .and_then(serde_json::Value::as_array)
            .map(|plans| plans.iter().map(Self::parse_plan_node).collect())
            .unwrap_or_default();

        QueryPlanNode {
            node_type,
            cost,
            rows,
            index_used,
            children,
        }
    }
}

impl Default for PostgresConnector {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DatabaseConnector for PostgresConnector {
    async fn connect(&mut self, connection_string: &str, options: &crate::core::types::ConnectOptions) -> Result<()> {
        let config: tokio_postgres::Config = connection_string
            .parse()
            .with_context(|| "Invalid PostgreSQL connection string")?;

        // Try connecting, but don't hang forever — cloud serverless Postgres (Neon) may cold-start.
        // Use a timeout so callers can surface a helpful message instead of appearing hung.
        let ssl_mode = config.get_ssl_mode();

        let connect_fut = async {
            if ssl_mode == SslMode::Require {
                let tls = native_tls::TlsConnector::builder()
                    .build()
                    .with_context(|| "Failed to build TLS connector")?;
                let tls_connector = postgres_native_tls::MakeTlsConnector::new(tls);
                let (client, connection) = config
                    .connect(tls_connector)
                    .await
                    .with_context(|| "Failed to connect to PostgreSQL with TLS")?;

                tokio::spawn(async move {
                    if let Err(e) = connection.await {
                        eprintln!("PostgreSQL connection error: {e}");
                    }
                });

                Ok(client)
            } else {
                let (client, connection) = config
                    .connect(NoTls)
                    .await
                    .with_context(|| "Failed to connect to PostgreSQL")?;

                tokio::spawn(async move {
                    if let Err(e) = connection.await {
                        eprintln!("PostgreSQL connection error: {e}");
                    }
                });

                Ok(client)
            }
        };

        // If connection takes longer than this, surface helpful guidance (Neon cold-starts can be slow).
        use tokio::time::{timeout, Duration};
        let to_secs = options.connect_timeout_secs.unwrap_or(25);
        match timeout(Duration::from_secs(to_secs), connect_fut).await {
            Ok(Ok(client)) => {
                self.client = Some(client);
                self.simple_mode = options.simple_mode;
                Ok(())
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err(anyhow!("PostgreSQL connection timed out (possible Neon cold-start). Try again with --verbose to see more details or increase the connection timeout).")),
        }
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.client = None;
        Ok(())
    }

    async fn test_connection(&self) -> Result<bool> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| anyhow!("PostgreSQL connection is not initialized"))?;
        if self.simple_mode {
            // use simple_query to avoid prepared statements
            let msgs = client
                .simple_query("SELECT 1")
                .await
                .with_context(|| "PostgreSQL simple health check failed")?;
            for m in msgs {
                if let tokio_postgres::SimpleQueryMessage::Row(row) = m {
                    if let Some(val) = row.get(0) {
                        if val == "1" {
                            return Ok(true);
                        }
                    }
                }
            }
            Ok(false)
        } else {
            let row = client
                .query_one("SELECT 1", &[])
                .await
                .with_context(|| "PostgreSQL health check failed")?;
            let value: i32 = row.get(0);
            Ok(value == 1)
        }
    }

    async fn introspect_schema(&self) -> Result<SchemaSnapshot> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| anyhow!("PostgreSQL connection is not initialized"))?;

        let table_rows = client
            .query(
                "
                SELECT table_name
                FROM information_schema.tables
                WHERE table_schema = 'public' AND table_type = 'BASE TABLE'
                ORDER BY table_name
                ",
                &[],
            )
            .await
            .with_context(|| "Failed to read PostgreSQL table list")?;

        let mut tables = Vec::new();
        for table_row in table_rows {
            let table_name: String = table_row.get("table_name");

            let column_rows = client
                .query(
                    "
                    SELECT column_name, data_type
                    FROM information_schema.columns
                    WHERE table_schema = 'public' AND table_name = $1
                    ORDER BY ordinal_position
                    ",
                    &[&table_name],
                )
                .await
                .with_context(|| format!("Failed to read columns for table '{table_name}'"))?;

            let columns = column_rows
                .iter()
                .map(|row| ColumnInfo {
                    name: row.get("column_name"),
                    data_type: row.get("data_type"),
                })
                .collect::<Vec<_>>();

            let index_rows = client
                .query(
                    "
                    SELECT indexname, indexdef
                    FROM pg_indexes
                    WHERE schemaname = 'public' AND tablename = $1
                    ORDER BY indexname
                    ",
                    &[&table_name],
                )
                .await
                .with_context(|| format!("Failed to read indexes for table '{table_name}'"))?;

            let indexes = index_rows
                .iter()
                .map(|row| {
                    let name: String = row.get("indexname");
                    let index_def: String = row.get("indexdef");
                    IndexInfo {
                        name,
                        columns: Self::parse_index_columns(&index_def),
                        is_unique: index_def.to_uppercase().contains(" UNIQUE "),
                    }
                })
                .collect::<Vec<_>>();

            // Fetch foreign keys for this table
            let fk_rows = client
                .query(
                    "
                    SELECT
                        kcu.constraint_name,
                        kcu.column_name,
                        ccu.table_name AS foreign_table_name,
                        ccu.column_name AS foreign_column_name,
                        kcu.ordinal_position
                    FROM information_schema.key_column_usage kcu
                    JOIN information_schema.constraint_column_usage ccu
                        ON kcu.constraint_name = ccu.constraint_name
                        AND kcu.table_schema = ccu.constraint_schema
                    WHERE kcu.table_schema = 'public' AND kcu.table_name = $1
                    ORDER BY kcu.constraint_name, kcu.ordinal_position
                    ",
                    &[&table_name],
                )
                .await
                .with_context(|| format!("Failed to read foreign keys for table '{table_name}'"))?;

            use std::collections::BTreeMap;
            let mut fk_map: BTreeMap<String, (String, Vec<(i32, String)>, Vec<(i32, String)>)> =
                BTreeMap::new();
            for row in fk_rows {
                let cname: String = row.get("constraint_name");
                let col: String = row.get("column_name");
                let ftable: String = row.get("foreign_table_name");
                let fcol: String = row.get("foreign_column_name");
                let pos: i32 = row.get("ordinal_position");
                let entry = fk_map
                    .entry(cname.clone())
                    .or_insert_with(|| (ftable.clone(), Vec::new(), Vec::new()));
                entry.1.push((pos, col));
                entry.2.push((pos, fcol));
            }

            let mut foreign_keys = Vec::new();
            for (cname, (ftable, mut cols, mut fcols)) in fk_map.into_iter() {
                cols.sort_by_key(|(p, _)| *p);
                fcols.sort_by_key(|(p, _)| *p);
                foreign_keys.push(crate::core::types::ForeignKeyInfo {
                    name: cname,
                    columns: cols.into_iter().map(|(_, c)| c).collect(),
                    referenced_table: ftable,
                    referenced_columns: fcols.into_iter().map(|(_, c)| c).collect(),
                });
            }

            tables.push(TableSchema {
                name: table_name,
                columns,
                indexes,
                foreign_keys,
            });
        }

        Ok(SchemaSnapshot { tables })
    }

    async fn explain_query(&self, query: &str) -> Result<QueryPlan> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| anyhow!("PostgreSQL connection is not initialized"))?;
        let explain_sql = format!("EXPLAIN (ANALYZE, FORMAT JSON) {query}");

        let parsed_opt: Option<serde_json::Value> = if self.simple_mode {
            let msgs = client
                .simple_query(&explain_sql)
                .await
                .with_context(|| "Failed to run PostgreSQL EXPLAIN (simple mode)")?;
            // collect text rows and parse first as JSON
            let mut parsed = None;
            for m in msgs {
                if let tokio_postgres::SimpleQueryMessage::Row(row) = m {
                    if let Some(s) = row.get(0) {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
                            parsed = Some(v);
                            break;
                        }
                    }
                }
            }
            parsed
        } else {
            let row = client
                .query_one(&explain_sql, &[])
                .await
                .with_context(|| "Failed to run PostgreSQL EXPLAIN")?;

            Some(row.get(0))
        };

        let raw = parsed_opt.unwrap_or_else(|| serde_json::json!({}));
        let plan_value = raw
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|obj| obj.get("Plan"))
            .cloned();

        let root = plan_value.as_ref().map(Self::parse_plan_node);

        Ok(QueryPlan {
            engine: "postgresql".to_string(),
            root,
            raw,
        })
    }

    fn database_type(&self) -> DatabaseType {
        DatabaseType::PostgreSQL
    }
}
