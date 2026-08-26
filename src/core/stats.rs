use crate::core::types::DatabaseType;

/// A row from pg_stat_statements or performance_schema.
#[derive(Debug, Clone)]
pub struct QueryStat {
    pub query: String,
    pub calls: i64,
    pub total_time_ms: f64,
    pub rows_returned: i64,
    pub queryid: Option<String>,
}

/// A table cardinality stat.
#[derive(Debug, Clone)]
pub struct TableStat {
    pub table_name: String,
    pub estimated_rows: i64,
    pub index_size_bytes: Option<i64>,
}

/// Overall database health snapshot.
#[derive(Debug, Clone)]
pub struct HealthSnapshot {
    pub database_type: DatabaseType,
    pub top_queries: Vec<QueryStat>,
    pub table_stats: Vec<TableStat>,
    pub stats_available: bool,
    pub stats_source: String,
}

/// SQL to fetch top queries from pg_stat_statements.
pub const PG_STAT_STATEMENTS_SQL: &str = "
SELECT query, calls, total_exec_time as total_time_ms, rows, queryid::text
FROM pg_stat_statements
ORDER BY total_exec_time DESC
LIMIT 20
";

/// SQL to check if pg_stat_statements is available.
pub const PG_STAT_EXISTS_SQL: &str = "
SELECT EXISTS (
    SELECT 1 FROM pg_extension WHERE extname = 'pg_stat_statements'
) as exists
";

/// SQL to fetch table cardinality from Postgres.
pub const PG_TABLE_STATS_SQL: &str = "
SELECT
    schemaname || '.' || relname as table_name,
    n_live_tup as estimated_rows
FROM pg_stat_user_tables
WHERE schemaname = 'public'
ORDER BY n_live_tup DESC
";

/// SQL to fetch top queries from MySQL performance_schema.
pub const MYSQL_PERF_SCHEMA_SQL: &str = "
SELECT
    DIGEST_TEXT as query,
    COUNT_STAR as calls,
    ROUND(SUM_TIMER_WAIT / 1e9, 3) as total_time_ms,
    SUM_ROWS_EXAMINED as rows_returned,
    DIGEST as queryid
FROM performance_schema.events_statements_summary_by_digest
WHERE SCHEMA_NAME IS NOT NULL
ORDER BY SUM_TIMER_WAIT DESC
LIMIT 20
";

/// SQL to check if MySQL performance_schema is enabled.
pub const MYSQL_PERF_SCHEMA_CHECK_SQL: &str = "
SELECT @@performance_schema as enabled
";

/// SQL to fetch table stats from MySQL.
pub const MYSQL_TABLE_STATS_SQL: &str = "
SELECT
    TABLE_NAME as table_name,
    TABLE_ROWS as estimated_rows,
    DATA_LENGTH + INDEX_LENGTH as total_size_bytes
FROM information_schema.TABLES
WHERE TABLE_SCHEMA = DATABASE()
ORDER BY TABLE_ROWS DESC
";

/// Parse the stats_available check result for Postgres.
pub fn parse_pg_stats_available(row: Option<&str>) -> bool {
    row.map(|v| v == "true" || v == "1" || v == "t")
        .unwrap_or(false)
}

/// Parse the stats_available check result for MySQL.
pub fn parse_mysql_stats_available(row: Option<&str>) -> bool {
    row.map(|v| v == "1" || v == "ON").unwrap_or(false)
}

fn column_index(columns: &[String], name: &str) -> Option<usize> {
    columns.iter().position(|c| c.eq_ignore_ascii_case(name))
}

/// Convert a generic read-only query result (via preview_rows) into QueryStats.
pub fn parse_query_stat_rows(preview: &crate::core::types::RowPreview) -> Vec<QueryStat> {
    let q = column_index(&preview.columns, "query");
    let calls = column_index(&preview.columns, "calls");
    let total = column_index(&preview.columns, "total_time_ms");
    let rows = column_index(&preview.columns, "rows_returned");
    let queryid = column_index(&preview.columns, "queryid");

    preview
        .rows
        .iter()
        .filter_map(|row| {
            let get = |idx: Option<usize>| idx.and_then(|i| row.get(i)).map(|s| s.as_str());
            Some(QueryStat {
                query: get(q)?.to_string(),
                calls: get(calls)
                    .and_then(|v| v.replace(',', "").parse().ok())
                    .unwrap_or(0),
                total_time_ms: get(total)
                    .and_then(|v| v.replace(',', "").parse().ok())
                    .unwrap_or(0.0),
                rows_returned: get(rows)
                    .and_then(|v| v.replace(',', "").parse().ok())
                    .unwrap_or(0),
                queryid: get(queryid).map(|s| s.to_string()),
            })
        })
        .collect()
}

/// Convert a generic read-only query result (via preview_rows) into TableStats.
pub fn parse_table_stat_rows(preview: &crate::core::types::RowPreview) -> Vec<TableStat> {
    let name = column_index(&preview.columns, "table_name");
    let est = column_index(&preview.columns, "estimated_rows");
    let size_cols = [
        column_index(&preview.columns, "index_size_bytes"),
        column_index(&preview.columns, "total_size_bytes"),
    ];

    preview
        .rows
        .iter()
        .filter_map(|row| {
            let get = |idx: Option<usize>| idx.and_then(|i| row.get(i)).map(|s| s.as_str());
            Some(TableStat {
                table_name: get(name)?.to_string(),
                estimated_rows: get(est)
                    .and_then(|v| v.replace(',', "").parse().ok())
                    .unwrap_or(0),
                index_size_bytes: size_cols
                    .iter()
                    .find_map(|&c| get(c))
                    .and_then(|v| v.replace(',', "").parse().ok()),
            })
        })
        .collect()
}
