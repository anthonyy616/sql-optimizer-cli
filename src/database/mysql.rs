use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use mysql_async::prelude::Queryable;
use mysql_async::{Opts, Pool};
use regex::Regex;

use crate::core::types::{
    ColumnInfo, DatabaseType, IndexInfo, QueryPlan, QueryPlanNode, SchemaSnapshot, TableSchema,
};
use crate::database::connection::DatabaseConnector;

pub struct MySqlConnector {
    pool: Option<Pool>,
    database_name: Option<String>,
}

impl MySqlConnector {
    pub fn new() -> Self {
        Self {
            pool: None,
            database_name: None,
        }
    }

    #[allow(dead_code)]
    fn parse_index_columns(index_def: &str) -> Vec<String> {
        let re = Regex::new(r"\((?P<cols>[^\)]+)\)").expect("valid mysql index regex");
        let cols = re
            .captures(index_def)
            .and_then(|caps| caps.name("cols"))
            .map(|m| m.as_str())
            .unwrap_or_default();

        cols.split(',')
            .map(|c| c.trim().trim_matches('`').to_string())
            .filter(|c| !c.is_empty())
            .collect()
    }

    fn parse_mysql_plan_node(node: &serde_json::Value) -> Option<QueryPlanNode> {
        let table_name = node
            .get("table_name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        let access_type = node
            .get("access_type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("scan");
        let rows = node
            .get("rows_examined_per_scan")
            .and_then(serde_json::Value::as_f64)
            .or_else(|| {
                node.get("rows_produced_per_join")
                    .and_then(serde_json::Value::as_f64)
            });
        let cost = node
            .get("cost_info")
            .and_then(|v| v.get("query_cost"))
            .and_then(serde_json::Value::as_str)
            .and_then(|s| s.parse::<f64>().ok());
        let index_used = node
            .get("key")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string);

        Some(QueryPlanNode {
            node_type: format!("{access_type}:{table_name}"),
            cost,
            rows,
            index_used,
            children: Vec::new(),
        })
    }
}

impl Default for MySqlConnector {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DatabaseConnector for MySqlConnector {
    async fn connect(&mut self, connection_string: &str) -> Result<()> {
        let opts = Opts::from_url(connection_string).with_context(|| {
            "Invalid MySQL connection URL. Expected mysql://user:pass@host:port/db"
        })?;

        let db_name = opts
            .db_name()
            .map(ToString::to_string)
            .ok_or_else(|| anyhow!("MySQL connection URL must include a database name"))?;

        let pool = Pool::new(opts);

        let mut conn = pool
            .get_conn()
            .await
            .with_context(|| "Failed to connect to MySQL")?;
        let ping: Option<u8> = conn
            .query_first("SELECT 1")
            .await
            .with_context(|| "MySQL health check failed")?;
        if ping != Some(1) {
            return Err(anyhow!("MySQL health check returned unexpected result"));
        }
        drop(conn);

        self.database_name = Some(db_name);
        self.pool = Some(pool);
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        if let Some(pool) = self.pool.take() {
            pool.disconnect().await?;
        }
        Ok(())
    }

    async fn test_connection(&self) -> Result<bool> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| anyhow!("MySQL pool is not initialized"))?;

        let mut conn = pool
            .get_conn()
            .await
            .with_context(|| "Failed to fetch MySQL connection from pool")?;
        let ping: Option<u8> = conn
            .query_first("SELECT 1")
            .await
            .with_context(|| "MySQL health check failed")?;
        drop(conn);

        Ok(ping == Some(1))
    }

    async fn introspect_schema(&self) -> Result<SchemaSnapshot> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| anyhow!("MySQL pool is not initialized"))?;
        let db_name = self
            .database_name
            .as_ref()
            .ok_or_else(|| anyhow!("MySQL database name is not initialized"))?;

        let mut conn = pool
            .get_conn()
            .await
            .with_context(|| "Failed to get MySQL connection for schema introspection")?;

        let table_rows: Vec<String> = conn
            .exec_map(
                "
                SELECT table_name
                FROM information_schema.tables
                WHERE table_schema = ? AND table_type = 'BASE TABLE'
                ORDER BY table_name
                ",
                (db_name.clone(),),
                |table_name: String| table_name,
            )
            .await
            .with_context(|| "Failed to read MySQL table list")?;

        let mut tables = Vec::new();
        for table_name in table_rows {
            let columns: Vec<ColumnInfo> = conn
                .exec_map(
                    "
                    SELECT column_name, data_type
                    FROM information_schema.columns
                    WHERE table_schema = ? AND table_name = ?
                    ORDER BY ordinal_position
                    ",
                    (db_name.clone(), table_name.clone()),
                    |(name, data_type): (String, String)| ColumnInfo { name, data_type },
                )
                .await
                .with_context(|| format!("Failed to read columns for table '{table_name}'"))?;

            let index_rows: Vec<(String, String, u8)> = conn
                .exec_map(
                    "
                    SELECT index_name, column_name, non_unique
                    FROM information_schema.statistics
                    WHERE table_schema = ? AND table_name = ?
                    ORDER BY index_name, seq_in_index
                    ",
                    (db_name.clone(), table_name.clone()),
                    |(index_name, column_name, non_unique): (String, String, u8)| {
                        (index_name, column_name, non_unique)
                    },
                )
                .await
                .with_context(|| format!("Failed to read indexes for table '{table_name}'"))?;

            let mut indexes_map = std::collections::BTreeMap::new();
            for (index_name, column_name, non_unique) in index_rows {
                let entry = indexes_map
                    .entry(index_name.clone())
                    .or_insert_with(|| IndexInfo {
                        name: index_name.clone(),
                        columns: Vec::new(),
                        is_unique: non_unique == 0,
                    });
                entry.columns.push(column_name);
            }
            let indexes = indexes_map.into_values().collect::<Vec<_>>();

            // Fetch foreign keys from information_schema.key_column_usage
            let fk_rows: Vec<(String, String, String, String, u64)> = conn
                .exec_map(
                    "
                    SELECT constraint_name, column_name, referenced_table_name, referenced_column_name, ordinal_position
                    FROM information_schema.key_column_usage
                    WHERE table_schema = ? AND table_name = ? AND referenced_table_name IS NOT NULL
                    ORDER BY constraint_name, ordinal_position
                    ",
                    (db_name.clone(), table_name.clone()),
                    |(constraint_name, column_name, ref_table, ref_column, ordinal_position)| {
                        (constraint_name, column_name, ref_table, ref_column, ordinal_position)
                    },
                )
                .await
                .with_context(|| format!("Failed to read foreign keys for table '{table_name}'"))?;

            use std::collections::BTreeMap;
            let mut fk_map: BTreeMap<String, (String, Vec<(u64, String)>, Vec<(u64, String)>)> =
                BTreeMap::new();
            for (cname, col, ref_table, ref_col, pos) in fk_rows {
                let entry = fk_map
                    .entry(cname.clone())
                    .or_insert_with(|| (ref_table.clone(), Vec::new(), Vec::new()));
                entry.1.push((pos, col));
                entry.2.push((pos, ref_col));
            }

            let mut foreign_keys = Vec::new();
            for (cname, (ref_table, mut cols, mut ref_cols)) in fk_map.into_iter() {
                cols.sort_by_key(|(p, _)| *p);
                ref_cols.sort_by_key(|(p, _)| *p);
                foreign_keys.push(crate::core::types::ForeignKeyInfo {
                    name: cname,
                    columns: cols.into_iter().map(|(_, c)| c).collect(),
                    referenced_table: ref_table,
                    referenced_columns: ref_cols.into_iter().map(|(_, c)| c).collect(),
                });
            }

            tables.push(TableSchema {
                name: table_name,
                columns,
                indexes,
                foreign_keys,
            });
        }

        drop(conn);
        Ok(SchemaSnapshot { tables })
    }

    async fn explain_query(&self, query: &str) -> Result<QueryPlan> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| anyhow!("MySQL pool is not initialized"))?;

        let mut conn = pool
            .get_conn()
            .await
            .with_context(|| "Failed to get MySQL connection for EXPLAIN")?;

        let explain_sql = format!("EXPLAIN FORMAT=JSON {query}");
        let raw_json: Option<String> = conn
            .query_first(explain_sql)
            .await
            .with_context(|| "Failed to run MySQL EXPLAIN")?;
        drop(conn);

        let raw_str = raw_json.ok_or_else(|| anyhow!("MySQL EXPLAIN returned no rows"))?;
        let raw: serde_json::Value =
            serde_json::from_str(&raw_str).with_context(|| "Failed to parse MySQL EXPLAIN JSON")?;

        let root = raw
            .get("query_block")
            .and_then(|qb| qb.get("table"))
            .and_then(Self::parse_mysql_plan_node)
            .or_else(|| {
                raw.get("query_block")
                    .and_then(|qb| qb.get("nested_loop"))
                    .and_then(serde_json::Value::as_array)
                    .and_then(|arr| arr.first())
                    .and_then(|entry| entry.get("table"))
                    .and_then(Self::parse_mysql_plan_node)
            });

        Ok(QueryPlan {
            engine: "mysql".to_string(),
            root,
            raw,
        })
    }

    fn database_type(&self) -> DatabaseType {
        DatabaseType::MySQL
    }
}
