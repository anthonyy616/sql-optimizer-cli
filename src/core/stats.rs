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
