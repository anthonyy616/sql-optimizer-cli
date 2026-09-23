use anyhow::{Context, Result};
use rusqlite::{params, params_from_iter, Connection};

use crate::core::fingerprint::fingerprint;

/// A stored query run record.
#[derive(Debug, Clone)]
pub struct QueryRun {
    pub fingerprint: String,
    pub query_text: String,
    pub timestamp: String,
    pub execution_time_ms: Option<u64>,
    pub rows_returned: Option<i64>,
    pub plan_summary: Option<String>,
    pub index_used: Option<String>,
    /// Connection/profile identity (may be absent for legacy rows).
    pub connection_id: Option<String>,
    /// Display label of the connection at run time (redacted URL or profile name).
    pub connection_label: Option<String>,
    /// False when the run failed (errors are recorded too).
    pub success: bool,
    /// Error message for failed runs.
    pub error: Option<String>,
}

/// Filters for the recent-runs listing used by the TUI History tab.
/// All set filters compose with AND.
#[derive(Debug, Clone, Default)]
pub struct RecentRunsFilter {
    /// Only runs recorded on this connection/profile ID.
    pub connection_id: Option<String>,
    /// Only runs with this query fingerprint.
    pub fingerprint: Option<String>,
    /// `Some(true)` = successful runs only, `Some(false)` = failed runs only.
    pub status: Option<bool>,
    /// Maximum number of rows returned.
    pub limit: usize,
}

/// A detected regression.
#[derive(Debug, Clone)]
pub struct Regression {
    pub fingerprint: String,
    pub query_text: String,
    pub regression_type: RegressionType,
    pub description: String,
    pub current_value: String,
    pub previous_value: String,
}

#[derive(Debug, Clone)]
pub enum RegressionType {
    Slower,
    LostIndex,
    MoreRowsScanned,
    NewIndexUsed,
}

/// State store for tracking query history.
pub struct StateStore {
    conn: Connection,
}

const RUN_COLUMNS: &str = "fingerprint, query_text, timestamp, execution_time_ms, rows_returned, plan_summary, index_used, connection_id, connection_label, success, error";

fn map_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueryRun> {
    Ok(QueryRun {
        fingerprint: row.get(0)?,
        query_text: row.get(1)?,
        timestamp: row.get(2)?,
        execution_time_ms: row.get(3)?,
        rows_returned: row.get(4)?,
        plan_summary: row.get(5)?,
        index_used: row.get(6)?,
        connection_id: row.get(7)?,
        connection_label: row.get(8)?,
        success: row.get::<_, Option<i64>>(9)?.unwrap_or(1) != 0,
        error: row.get(10)?,
    })
}

impl StateStore {
    /// Open or create the state store at the given path.
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("Failed to open state store at '{}'", path))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS query_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                fingerprint TEXT NOT NULL,
                query_text TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                execution_time_ms INTEGER,
                rows_returned INTEGER,
                plan_summary TEXT,
                index_used TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_fingerprint ON query_runs(fingerprint);
            CREATE INDEX IF NOT EXISTS idx_timestamp ON query_runs(timestamp);
            ",
        )
        .context("Failed to initialize state store schema")?;

        // Migration: add per-run connection identity and status columns when
        // upgrading from a pre-workspace store. Each ALTER fails harmlessly
        // when the column already exists.
        for alter in [
            "ALTER TABLE query_runs ADD COLUMN connection_id TEXT;",
            "ALTER TABLE query_runs ADD COLUMN connection_label TEXT;",
            "ALTER TABLE query_runs ADD COLUMN success INTEGER NOT NULL DEFAULT 1;",
            "ALTER TABLE query_runs ADD COLUMN error TEXT;",
        ] {
            if let Err(e) = conn.execute_batch(alter) {
                let msg = e.to_string();
                if !msg.contains("duplicate column name") {
                    return Err(e).context("Failed to migrate state store schema");
                }
            }
        }

        Ok(Self { conn })
    }

    /// Open the default state store in `.sql-optimizer/history.sqlite`.
    pub fn open_default() -> Result<Self> {
        let dir = std::path::Path::new(".sql-optimizer");
        if !dir.exists() {
            std::fs::create_dir_all(dir).context("Failed to create .sql-optimizer directory")?;
        }
        Self::open(".sql-optimizer/history.sqlite")
    }

    /// Check if a default state store exists.
    pub fn default_exists() -> bool {
        std::path::Path::new(".sql-optimizer/history.sqlite").exists()
    }

    /// Record a query run with full workspace metadata. Never store secrets —
    /// `connection_id`/`connection_label` are opaque identity/display values.
    #[allow(clippy::too_many_arguments)]
    pub fn record_run(
        &self,
        query: &str,
        execution_time_ms: Option<u64>,
        rows_returned: Option<i64>,
        plan_summary: Option<&str>,
        index_used: Option<&str>,
        connection_id: Option<&str>,
        connection_label: Option<&str>,
        success: bool,
        error: Option<&str>,
    ) -> Result<()> {
        let fp = fingerprint(query);
        let ts = chrono::Utc::now().to_rfc3339();

        self.conn
            .execute(
                "INSERT INTO query_runs (fingerprint, query_text, timestamp, execution_time_ms, rows_returned, plan_summary, index_used, connection_id, connection_label, success, error)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    fp,
                    query,
                    ts,
                    execution_time_ms,
                    rows_returned,
                    plan_summary,
                    index_used,
                    connection_id,
                    connection_label,
                    success,
                    error,
                ],
            )
            .context("Failed to record query run")?;

        Ok(())
    }

    /// Backwards-compatible recording used by the CLI path (no connection identity).
    pub fn record_run_basic(
        &self,
        query: &str,
        execution_time_ms: Option<u64>,
        rows_returned: Option<i64>,
        plan_summary: Option<&str>,
        index_used: Option<&str>,
    ) -> Result<()> {
        self.record_run(
            query,
            execution_time_ms,
            rows_returned,
            plan_summary,
            index_used,
            None,
            None,
            true,
            None,
        )
    }

    /// Get the last N runs for a given fingerprint.
    pub fn get_history(&self, fp: &str, limit: usize) -> Result<Vec<QueryRun>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {RUN_COLUMNS}
             FROM query_runs
             WHERE fingerprint = ?1
             ORDER BY timestamp DESC
             LIMIT ?2"
        ))?;

        let rows = stmt.query_map(params![fp, limit as i64], map_run)?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get the most recent run for a fingerprint.
    pub fn get_last_run(&self, fp: &str) -> Result<Option<QueryRun>> {
        let history = self.get_history(fp, 1)?;
        Ok(history.into_iter().next())
    }

    /// Get the most recent N runs across all fingerprints (newest first).
    pub fn get_recent_runs(&self, limit: usize) -> Result<Vec<QueryRun>> {
        self.get_recent_runs_filtered(RecentRunsFilter {
            limit,
            ..Default::default()
        })
    }

    /// Filtered/paginated run listing used by the TUI History tab.
    pub fn get_recent_runs_filtered(&self, filter: RecentRunsFilter) -> Result<Vec<QueryRun>> {
        let mut where_clauses: Vec<&str> = Vec::new();
        let mut values: Vec<rusqlite::types::Value> = Vec::new();

        if let Some(conn_id) = &filter.connection_id {
            where_clauses.push("connection_id = ?");
            values.push(conn_id.clone().into());
        }
        if let Some(fp) = &filter.fingerprint {
            where_clauses.push("fingerprint = ?");
            values.push(fp.clone().into());
        }
        if let Some(status) = filter.status {
            where_clauses.push("success = ?");
            values.push((if status { 1 } else { 0 }).into());
        }
        values.push((filter.limit as i64).into());

        let where_sql = if where_clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", where_clauses.join(" AND "))
        };

        let mut stmt = self.conn.prepare(&format!(
            "SELECT {RUN_COLUMNS}
             FROM query_runs
             {where_sql}
             ORDER BY timestamp DESC
             LIMIT ?"
        ))?;

        let rows = stmt.query_map(params_from_iter(values.iter()), map_run)?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Detect regressions by comparing current state against history.
    pub fn detect_regressions(
        &self,
        query: &str,
        current_time_ms: Option<u64>,
        current_plan_summary: Option<&str>,
        current_index_used: Option<&str>,
    ) -> Result<Vec<Regression>> {
        let fp = fingerprint(query);
        let history = self.get_history(&fp, 5)?;

        if history.len() < 2 {
            // Need at least 2 historical runs to compare
            return Ok(Vec::new());
        }

        // Use the most recent previous run (not the current one)
        let previous = &history[0]; // Most recent historical run

        let mut regressions = Vec::new();

        // Check for slower execution
        if let (Some(current_time), Some(prev_time)) = (current_time_ms, previous.execution_time_ms)
        {
            if prev_time > 0 {
                let slowdown_pct =
                    ((current_time as f64 - prev_time as f64) / prev_time as f64) * 100.0;
                if slowdown_pct > 20.0 {
                    regressions.push(Regression {
                        fingerprint: fp.clone(),
                        query_text: query.to_string(),
                        regression_type: RegressionType::Slower,
                        description: format!(
                            "Query got {:.1}% slower (was {}ms, now {}ms)",
                            slowdown_pct, prev_time, current_time
                        ),
                        current_value: format!("{}ms", current_time),
                        previous_value: format!("{}ms", prev_time),
                    });
                }
            }
        }

        // Check for lost index
        match (&current_index_used, &previous.index_used) {
            (None, Some(prev_idx)) => {
                regressions.push(Regression {
                    fingerprint: fp.clone(),
                    query_text: query.to_string(),
                    regression_type: RegressionType::LostIndex,
                    description: format!(
                        "Query previously used index '{}' but no longer uses an index",
                        prev_idx
                    ),
                    current_value: "no index".to_string(),
                    previous_value: prev_idx.clone(),
                });
            }
            (Some(curr_idx), Some(prev_idx)) if *curr_idx != *prev_idx => {
                regressions.push(Regression {
                    fingerprint: fp.clone(),
                    query_text: query.to_string(),
                    regression_type: RegressionType::NewIndexUsed,
                    description: format!(
                        "Index usage changed from '{}' to '{}' — verify this is expected",
                        prev_idx, curr_idx
                    ),
                    current_value: curr_idx.to_string(),
                    previous_value: prev_idx.to_string(),
                });
            }
            _ => {}
        }

        // Check for more rows scanned
        if let (Some(current_rows), Some(prev_rows)) =
            (current_plan_summary, &previous.plan_summary)
        {
            // Heuristic: extract row count from plan summary
            let current_row_count = extract_row_estimate(current_rows);
            let prev_row_count = extract_row_estimate(prev_rows);
            if let (Some(curr), Some(prev)) = (current_row_count, prev_row_count) {
                if prev > 0.0 && curr / prev > 1.5 {
                    regressions.push(Regression {
                        fingerprint: fp.clone(),
                        query_text: query.to_string(),
                        regression_type: RegressionType::MoreRowsScanned,
                        description: format!(
                            "Rows scanned increased from ~{} to ~{:.0} ({:.1}x more)",
                            prev,
                            curr,
                            curr / prev
                        ),
                        current_value: format!("{:.0}", curr),
                        previous_value: format!("{:.0}", prev),
                    });
                }
            }
        }

        Ok(regressions)
    }
}

/// Try to extract a row count from a plan summary string like "~40000 rows".
fn extract_row_estimate(summary: &str) -> Option<f64> {
    let re = regex::Regex::new(r"~?(\d[\d,]*)\s*rows?").ok()?;
    let caps = re.captures(summary)?;
    let num_str = caps.get(1)?.as_str().replace(',', "");
    num_str.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_in(dir: &tempfile::TempDir) -> StateStore {
        let path = dir.path().join("history.sqlite");
        StateStore::open(path.to_str().unwrap()).unwrap()
    }

    #[test]
    fn record_and_reload_runs_with_connection_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(&dir);
        store
            .record_run(
                "SELECT * FROM users WHERE id = 1",
                Some(12),
                Some(1),
                Some("~1 row"),
                Some("idx_users_id"),
                Some("profile-abc"),
                Some("Local Postgres"),
                true,
                None,
            )
            .unwrap();
        store
            .record_run(
                "SELECT * FROM missing_table",
                None,
                None,
                None,
                None,
                Some("profile-abc"),
                Some("Local Postgres"),
                false,
                Some("no such table"),
            )
            .unwrap();

        let runs = store.get_recent_runs(10).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].connection_id.as_deref(), Some("profile-abc"));
        assert_eq!(runs[0].connection_label.as_deref(), Some("Local Postgres"));
        assert!(!runs[0].success);
        assert_eq!(runs[0].error.as_deref(), Some("no such table"));
        assert!(runs[1].success);
    }

    #[test]
    fn filters_compose_with_and() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(&dir);
        for (conn, ok) in [("a", true), ("a", false), ("b", true)] {
            store
                .record_run(
                    "SELECT 1",
                    Some(1),
                    None,
                    None,
                    None,
                    Some(conn),
                    Some(conn),
                    ok,
                    if ok { None } else { Some("boom") },
                )
                .unwrap();
        }

        let only_a_ok = store
            .get_recent_runs_filtered(RecentRunsFilter {
                connection_id: Some("a".into()),
                status: Some(true),
                limit: 100,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(only_a_ok.len(), 1);
        assert_eq!(only_a_ok[0].connection_id.as_deref(), Some("a"));
        assert!(only_a_ok[0].success);

        let only_failed = store
            .get_recent_runs_filtered(RecentRunsFilter {
                status: Some(false),
                limit: 100,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(only_failed.len(), 1);

        // Fingerprint filter matches everything recorded from "SELECT 1".
        let fp = fingerprint("SELECT 1");
        let by_fp = store
            .get_recent_runs_filtered(RecentRunsFilter {
                fingerprint: Some(fp),
                limit: 100,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(by_fp.len(), 3);
    }

    #[test]
    fn legacy_store_without_new_columns_migrates_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE query_runs (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    fingerprint TEXT NOT NULL,
                    query_text TEXT NOT NULL,
                    timestamp TEXT NOT NULL,
                    execution_time_ms INTEGER,
                    rows_returned INTEGER,
                    plan_summary TEXT,
                    index_used TEXT
                );
                INSERT INTO query_runs (fingerprint, query_text, timestamp)
                VALUES ('fp', 'SELECT 1', '2026-01-01T00:00:00Z');",
            )
            .unwrap();
        }
        let store = StateStore::open(path.to_str().unwrap()).unwrap();
        let runs = store.get_recent_runs(10).unwrap();
        assert_eq!(runs.len(), 1);
        assert!(runs[0].connection_id.is_none());
        assert!(runs[0].success, "legacy rows default to success");
    }

    #[test]
    fn record_run_basic_still_works() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_in(&dir);
        store
            .record_run_basic("SELECT 2", Some(5), None, None, None)
            .unwrap();
        let runs = store.get_recent_runs(5).unwrap();
        assert_eq!(runs.len(), 1);
        assert!(runs[0].connection_id.is_none());
    }
}
