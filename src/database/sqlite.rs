use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use regex::Regex;
use rusqlite::{types::ValueRef, Connection};

use crate::core::types::{
    ColumnInfo, DatabaseType, IndexInfo, QueryPlan, QueryPlanNode, RowPreview, SchemaSnapshot,
    TableSchema,
};
use crate::database::connection::DatabaseConnector;
use crate::utils::parser::{ensure_read_only_select, preview_sql};

pub struct SqliteConnector {
    db_path: Option<String>,
}

impl SqliteConnector {
    pub fn new() -> Self {
        Self { db_path: None }
    }

    fn parse_path(connection_string: &str) -> Result<String> {
        if connection_string == "sqlite::memory:" {
            return Ok(":memory:".to_string());
        }

        if let Some(path) = connection_string.strip_prefix("sqlite://") {
            if path.is_empty() {
                return Err(anyhow!("SQLite URL must include a database path"));
            }
            return Ok(path.to_string());
        }

        if connection_string.ends_with(".db") || connection_string.ends_with(".sqlite") {
            return Ok(connection_string.to_string());
        }

        Err(anyhow!(
            "Invalid SQLite connection string. Use sqlite::memory: or sqlite://path/to/file.db"
        ))
    }

    fn open_connection(path: &str) -> Result<Connection> {
        Connection::open(path)
            .with_context(|| format!("Failed to open SQLite database at '{path}'"))
    }

    fn parse_explain_detail(detail: &str) -> QueryPlanNode {
        let index_regex =
            Regex::new(r"USING (?:COVERING )?INDEX ([^ ]+)").expect("valid sqlite explain regex");
        let index_used = index_regex
            .captures(detail)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str().to_string());

        let upper = detail.to_uppercase();
        let node_type = if upper.starts_with("SCAN") {
            "scan"
        } else if upper.starts_with("SEARCH") {
            "search"
        } else {
            "operation"
        }
        .to_string();

        QueryPlanNode {
            node_type,
            cost: None,
            rows: None,
            index_used,
            children: Vec::new(),
        }
    }
}

impl Default for SqliteConnector {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl DatabaseConnector for SqliteConnector {
    async fn connect(
        &mut self,
        connection_string: &str,
        _options: &crate::core::types::ConnectOptions,
    ) -> Result<()> {
        let path = Self::parse_path(connection_string)?;
        let path_for_open = path.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let conn = Self::open_connection(&path_for_open)?;
            conn.execute_batch("SELECT 1;")
                .with_context(|| "SQLite health check failed")?;
            Ok(())
        })
        .await
        .with_context(|| "SQLite connection task failed")??;

        self.db_path = Some(path);
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        self.db_path = None;
        Ok(())
    }

    async fn test_connection(&self) -> Result<bool> {
        let path = self
            .db_path
            .as_ref()
            .ok_or_else(|| anyhow!("SQLite connection is not initialized"))?
            .to_string();

        let ok = tokio::task::spawn_blocking(move || -> Result<bool> {
            let conn = Self::open_connection(&path)?;
            let mut stmt = conn.prepare("SELECT 1")?;
            let mut rows = stmt.query([])?;
            Ok(rows.next()?.is_some())
        })
        .await
        .with_context(|| "SQLite test task failed")??;

        Ok(ok)
    }

    async fn introspect_schema(&self) -> Result<SchemaSnapshot> {
        let path = self
            .db_path
            .as_ref()
            .ok_or_else(|| anyhow!("SQLite connection is not initialized"))?
            .to_string();

        tokio::task::spawn_blocking(move || -> Result<SchemaSnapshot> {
            let conn = Self::open_connection(&path)?;

            let mut table_stmt = conn.prepare(
                "
                SELECT name
                FROM sqlite_master
                WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
                ORDER BY name
                ",
            )?;
            let table_names = table_stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            let mut tables = Vec::new();
            for table_name in table_names {
                let mut col_stmt = conn.prepare(&format!("PRAGMA table_info('{table_name}')"))?;
                let columns = col_stmt
                    .query_map([], |row| {
                        Ok(ColumnInfo {
                            name: row.get(1)?,
                            data_type: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                        })
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;

                let mut idx_list_stmt =
                    conn.prepare(&format!("PRAGMA index_list('{table_name}')"))?;
                let index_rows = idx_list_stmt
                    .query_map([], |row| {
                        Ok((row.get::<_, String>(1)?, row.get::<_, i64>(2)? == 1))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;

                let mut indexes = Vec::new();
                for (index_name, is_unique) in index_rows {
                    let mut idx_col_stmt =
                        conn.prepare(&format!("PRAGMA index_info('{index_name}')"))?;
                    let index_columns = idx_col_stmt
                        .query_map([], |row| row.get::<_, String>(2))?
                        .collect::<std::result::Result<Vec<_>, _>>()?;

                    indexes.push(IndexInfo {
                        name: index_name,
                        columns: index_columns,
                        is_unique,
                    });
                }

                let mut fk_stmt =
                    conn.prepare(&format!("PRAGMA foreign_key_list('{table_name}')"))?;
                let fk_rows = fk_stmt
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    })?
                    .collect::<std::result::Result<Vec<_>, _>>()?;

                use std::collections::BTreeMap;
                let mut fk_map: BTreeMap<i64, (String, Vec<(i64, String)>, Vec<(i64, String)>)> =
                    BTreeMap::new();
                for (id, seq, ref_table, from_col, to_col) in fk_rows {
                    let entry = fk_map
                        .entry(id)
                        .or_insert_with(|| (ref_table.clone(), Vec::new(), Vec::new()));
                    entry.1.push((seq, from_col.clone()));
                    entry.2.push((seq, to_col.clone()));
                }

                let mut foreign_keys = Vec::new();
                for (id, (ref_table, cols, ref_cols)) in fk_map.into_iter() {
                    let mut cols_sorted = cols;
                    cols_sorted.sort_by_key(|(s, _)| *s);
                    let mut ref_cols_sorted = ref_cols;
                    ref_cols_sorted.sort_by_key(|(s, _)| *s);
                    foreign_keys.push(crate::core::types::ForeignKeyInfo {
                        name: format!("fk_{}_{}", table_name, id),
                        columns: cols_sorted.into_iter().map(|(_, c)| c).collect(),
                        referenced_table: ref_table,
                        referenced_columns: ref_cols_sorted.into_iter().map(|(_, c)| c).collect(),
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
        })
        .await
        .with_context(|| "SQLite schema task failed")?
    }

    async fn explain_query(&self, query: &str) -> Result<QueryPlan> {
        let path = self
            .db_path
            .as_ref()
            .ok_or_else(|| anyhow!("SQLite connection is not initialized"))?
            .to_string();
        let sql = query.to_string();

        tokio::task::spawn_blocking(move || -> Result<QueryPlan> {
            let conn = Self::open_connection(&path)?;
            let explain_sql = format!("EXPLAIN QUERY PLAN {sql}");
            let mut stmt = conn.prepare(&explain_sql)?;
            let details = stmt
                .query_map([], |row| row.get::<_, String>(3))?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            let root = details
                .first()
                .map(|detail| Self::parse_explain_detail(detail));
            let raw = serde_json::json!({ "details": details });

            Ok(QueryPlan {
                engine: "sqlite".to_string(),
                root,
                raw,
            })
        })
        .await
        .with_context(|| "SQLite explain task failed")?
    }

    async fn preview_rows(&self, query: &str, limit: usize) -> Result<RowPreview> {
        let path = self
            .db_path
            .as_ref()
            .ok_or_else(|| anyhow!("SQLite connection is not initialized"))?
            .to_string();

        ensure_read_only_select(query, DatabaseType::SQLite)?;

        let preview_sql = preview_sql(query, limit);

        tokio::task::spawn_blocking(move || -> Result<RowPreview> {
            let conn = Self::open_connection(&path)?;
            let mut stmt = conn.prepare(&preview_sql)?;
            let columns = stmt
                .column_names()
                .into_iter()
                .map(|name| name.to_string())
                .collect::<Vec<_>>();
            let rows = stmt
                .query_map([], |row| {
                    let mut values = Vec::new();
                    for idx in 0..row.as_ref().column_count() {
                        values.push(sqlite_value_to_string(row.get_ref(idx)?));
                    }
                    Ok(values)
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            Ok(RowPreview {
                columns,
                truncated: rows.len() >= limit,
                rows,
                limit,
            })
        })
        .await
        .with_context(|| "SQLite row preview task failed")?
    }

    fn database_type(&self) -> DatabaseType {
        DatabaseType::SQLite
    }
}

fn sqlite_value_to_string(value: ValueRef<'_>) -> String {
    match value {
        ValueRef::Null => String::new(),
        ValueRef::Integer(value) => value.to_string(),
        ValueRef::Real(value) => value.to_string(),
        ValueRef::Text(bytes) => String::from_utf8_lossy(bytes).to_string(),
        ValueRef::Blob(bytes) => format!("<{} bytes>", bytes.len()),
    }
}
