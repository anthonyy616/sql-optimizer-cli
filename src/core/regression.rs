use anyhow::{Context, Result};
use rusqlite::{params, Connection};

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

    /// Record a query run.
    pub fn record_run(
        &self,
        query: &str,
        execution_time_ms: Option<u64>,
        rows_returned: Option<i64>,
        plan_summary: Option<&str>,
        index_used: Option<&str>,
    ) -> Result<()> {
        let fp = fingerprint(query);
        let ts = chrono::Utc::now().to_rfc3339();

        self.conn
            .execute(
                "INSERT INTO query_runs (fingerprint, query_text, timestamp, execution_time_ms, rows_returned, plan_summary, index_used)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![fp, query, ts, execution_time_ms, rows_returned, plan_summary, index_used],
            )
            .context("Failed to record query run")?;

        Ok(())
    }

    /// Get the last N runs for a given fingerprint.
    pub fn get_history(&self, fp: &str, limit: usize) -> Result<Vec<QueryRun>> {
        let mut stmt = self.conn.prepare(
            "SELECT fingerprint, query_text, timestamp, execution_time_ms, rows_returned, plan_summary, index_used
             FROM query_runs
             WHERE fingerprint = ?1
             ORDER BY timestamp DESC
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![fp, limit as i64], |row| {
            Ok(QueryRun {
                fingerprint: row.get(0)?,
                query_text: row.get(1)?,
                timestamp: row.get(2)?,
                execution_time_ms: row.get(3)?,
                rows_returned: row.get(4)?,
                plan_summary: row.get(5)?,
                index_used: row.get(6)?,
            })
        })?;

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
        let mut stmt = self.conn.prepare(
            "SELECT fingerprint, query_text, timestamp, execution_time_ms, rows_returned, plan_summary, index_used
             FROM query_runs
             ORDER BY timestamp DESC
             LIMIT ?1",
        )?;

        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(QueryRun {
                fingerprint: row.get(0)?,
                query_text: row.get(1)?,
                timestamp: row.get(2)?,
                execution_time_ms: row.get(3)?,
                rows_returned: row.get(4)?,
                plan_summary: row.get(5)?,
                index_used: row.get(6)?,
            })
        })?;

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
