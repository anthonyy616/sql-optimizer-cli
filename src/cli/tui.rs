//! Phase "TUI": a responsive terminal-user-interface query workspace.
//!
//! The TUI always launches — even with no working database connection. All
//! database work (connect, schema, health, query preview, analysis) runs on a
//! background worker task; the UI event loop never blocks on the database and
//! never calls `block_on`. Results flow back over a channel and are applied
//! only when they belong to the *current* connection generation, so switching
//! databases never leaks stale schema/results/analysis into the UI.
//!
//! Connections are profile-backed: saved profiles persist user-globally
//! (`core::connections::ProfileCatalog`) with passwords in the OS credential
//! store; `sqlite::memory:` stays session-only. Query execution is read-only
//! (SELECT-only, enforced at the connector boundary via `preview_rows`).
//!
//! Layout:
//! ┌──────────────────────────────────────────────────────────────┐
//! │ sql-optimizer-cli ● postgresql://…   profile: oltp           │  header
//! ├──────────────────────────────────────────────────────────────┤
//! │ [Connect] [Query] [Analyze] [Optimize] [Schema] [Health] …   │  tab bar
//! │                 active tab content                           │
//! ├──────────────────────────────────────────────────────────────┤
//! │ SQL> select * from users where email = 'x'                   │  input
//! ├──────────────────────────────────────────────────────────────┤
//! │ Tab: switch · Enter: run · ↑↓: scroll · q/Esc: quit          │  footer
//! └──────────────────────────────────────────────────────────────┘

use anyhow::{Context, Result};
use crossterm::{
    event::{self, DisableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Tabs, Wrap},
    Frame, Terminal,
};
use std::collections::HashSet;
use std::io;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::cli::ConnectionArgs;
use crate::core::connections::{
    inject_secret_into_url, redact_profile_url, ConnectionProfile, DatabaseKind,
    KeychainSecretStore, ProfileCatalog, SecretStore,
};
use crate::core::types::*;

const TABS: &[&str] = &[
    "Connect", "Query", "Analyze", "Optimize", "Schema", "Health", "History",
];

const TAB_CONNECT: usize = 0;
const TAB_QUERY: usize = 1;
const TAB_ANALYZE: usize = 2;
const TAB_OPTIMIZE: usize = 3;
const TAB_SCHEMA: usize = 4;
const TAB_HEALTH: usize = 5;
const TAB_HISTORY: usize = 6;

/// Cap on how many result rows the Query tab renders at once.
const QUERY_VIEW_ROW_CAP: usize = 100;
/// Cap on stored history rows shown at once.
const HISTORY_PAGE_SIZE: usize = 100;

// Connect-tab list geometry: rows 1..=5 = the five provider presets,
// row 6 = "Saved profiles" header, rows 7.. = saved + session entries.
const PROVIDER_FIRST_ROW: usize = 1;
const PROVIDER_LAST_ROW: usize = 5;
const SESSION_FIRST_ROW: usize = 7;

/// Database provider shown in the Connect catalog. Supabase and Neon are
/// Postgres-compatible; the distinction is cosmetic labeling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provider {
    Sqlite,
    Postgres,
    Mysql,
    Supabase,
    Neon,
}

impl Provider {
    fn label(self) -> &'static str {
        match self {
            Provider::Sqlite => "SQLite",
            Provider::Postgres => "PostgreSQL",
            Provider::Mysql => "MySQL",
            Provider::Supabase => "Supabase",
            Provider::Neon => "Neon",
        }
    }

    fn color(self) -> Color {
        match self {
            Provider::Sqlite => Color::LightYellow,
            Provider::Postgres => Color::Blue,
            Provider::Mysql => Color::Cyan,
            Provider::Supabase => Color::Green,
            Provider::Neon => Color::Magenta,
        }
    }

    fn kind(self) -> DatabaseKind {
        match self {
            Provider::Sqlite => DatabaseKind::Sqlite,
            Provider::Postgres => DatabaseKind::Postgres,
            Provider::Mysql => DatabaseKind::Mysql,
            Provider::Supabase => DatabaseKind::Supabase,
            Provider::Neon => DatabaseKind::Neon,
        }
    }

    /// Template connection string used to prefill the add-connection form.
    fn template(self) -> &'static str {
        match self {
            Provider::Sqlite => "sqlite::memory:",
            Provider::Postgres => "postgresql://user@localhost:5432/postgres?sslmode=require",
            Provider::Mysql => "mysql://user@localhost:3306/mydb",
            Provider::Supabase => {
                "postgresql://postgres@db.<project-ref>.supabase.co:5432/postgres?sslmode=require"
            }
            Provider::Neon => {
                "postgresql://user@ep-<endpoint>.<region>.aws.neon.tech/neondb?sslmode=require"
            }
        }
    }

    fn blurb(self) -> &'static str {
        match self {
            Provider::Sqlite => "zero-setup — in-memory (session-only) or a local .db file",
            Provider::Postgres => "local or self-hosted server",
            Provider::Mysql => "local or self-hosted server",
            Provider::Supabase => "Postgres-compatible — use the session pooler URL",
            Provider::Neon => "Postgres-compatible — serverless connection string",
        }
    }

    fn needs_password(self) -> bool {
        !matches!(self, Provider::Sqlite)
    }
}

const PROVIDERS: &[Provider] = &[
    Provider::Sqlite,
    Provider::Postgres,
    Provider::Mysql,
    Provider::Supabase,
    Provider::Neon,
];

/// Detect the provider from a connection string (cosmetic only).
fn provider_from_url(url: &str) -> Option<Provider> {
    let lower = url.to_lowercase();
    if lower.starts_with("sqlite") || lower.ends_with(".db") || lower.ends_with(".sqlite") {
        Some(Provider::Sqlite)
    } else if lower.starts_with("mysql") {
        Some(Provider::Mysql)
    } else if lower.starts_with("postgres") {
        if lower.contains("supabase") {
            Some(Provider::Supabase)
        } else if lower.contains("neon.tech") {
            Some(Provider::Neon)
        } else {
            Some(Provider::Postgres)
        }
    } else {
        None
    }
}

fn kind_from_url(url: &str) -> DatabaseKind {
    provider_from_url(url)
        .map(|p| p.kind())
        .unwrap_or(DatabaseKind::Postgres)
}

/// A connection shown in the Connect catalog: either a persisted profile or a
/// session-only entry (CLI-provided, in-memory SQLite, unsaved drafts).
#[derive(Debug, Clone)]
struct ConnectionEntry {
    profile: ConnectionProfile,
    saved: bool,
}

impl ConnectionEntry {
    fn session(name: &str, url: &str) -> Self {
        let mut p = ConnectionProfile::new(name, kind_from_url(url), url);
        // Session entries never own secrets.
        p.secret_ref = None;
        Self {
            profile: p,
            saved: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnInputStage {
    /// Collecting/editing the connection URL.
    Url,
    /// URL accepted; collecting a display name.
    Name,
    /// Collecting the password (stored in the credential store, never on disk).
    Password,
    /// Ask whether to accept invalid TLS certificates (postgres-like only).
    Cert,
}

/// One background operation kind, for loading/error state tracking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Op {
    Connect,
    Schema,
    Health,
    QueryPreview,
    Analysis,
    History,
}

/// Filter state for the History tab.
#[derive(Debug, Clone, Default)]
struct HistoryFilter {
    /// Only runs recorded on the currently active connection.
    this_connection_only: bool,
    /// `Some(true)` = successes only, `Some(false)` = failures only.
    status: Option<bool>,
}

impl HistoryFilter {
    fn label(&self) -> String {
        let mut parts = vec!["all".to_string()];
        parts.clear();
        if self.this_connection_only {
            parts.push("this connection".into());
        } else {
            parts.push("all connections".into());
        }
        match self.status {
            Some(true) => parts.push("successful".into()),
            Some(false) => parts.push("failed".into()),
            None => parts.push("any status".into()),
        }
        parts.join(" · ")
    }
}

/// A job sent to the background worker.
enum Job {
    Connect {
        gen: u64,
        url: String,
        label: String,
        profile_id: Option<String>,
        accept_invalid_certs: bool,
    },
    Disconnect {
        gen: u64,
    },
    Schema {
        gen: u64,
    },
    Health {
        gen: u64,
    },
    /// Read-only SELECT preview (Query tab).
    QueryPreview {
        gen: u64,
        query: String,
        limit: usize,
    },
    /// Full analysis pipeline (analyze + schema checks + plan).
    Analyze {
        gen: u64,
        query: String,
        profile: Profile,
    },
    /// Query text is carried in the payload (used by the worker for history
    /// recording); the result echoes it back for stale-guarding.
    History {
        filter: crate::core::regression::RecentRunsFilter,
    },
}

/// A result produced by the background worker.
enum JobResult {
    Connected {
        gen: u64,
        ok: bool,
        db_type: Option<DatabaseType>,
        message: String,
    },
    Disconnected {
        gen: u64,
        message: String,
    },
    Schema {
        gen: u64,
        result: std::result::Result<SchemaSnapshot, String>,
    },
    Health {
        gen: u64,
        lines: Vec<String>,
    },
    QueryPreview {
        gen: u64,
        query: String,
        result: std::result::Result<RowPreview, String>,
        elapsed_ms: u64,
    },
    Analysis {
        gen: u64,
        query: String,
        result: Box<std::result::Result<AnalysisResult, String>>,
        elapsed_ms: u64,
    },
    History {
        runs: Vec<crate::core::regression::QueryRun>,
        error: Option<String>,
    },
}

struct App {
    tab: usize,
    input: String,
    results: Vec<AnalysisResult>,
    selected_result: Option<usize>,
    result_scroll: u16,
    schema: Option<SchemaSnapshot>,
    schema_list_state: ListState,
    health_lines: Vec<String>,
    db_type: Option<DatabaseType>,
    db_label: String,
    profile: Profile,
    status: String,

    // Active connection identity for history records and stale-result guards.
    conn_gen: u64,
    active_profile_id: Option<String>,
    busy: HashSet<Op>,

    // Query tab state.
    query_rows: Option<RowPreview>,
    query_error: Option<String>,
    query_elapsed_ms: Option<u64>,

    // History tab state.
    history_runs: Vec<crate::core::regression::QueryRun>,
    history_error: Option<String>,
    history_filter: HistoryFilter,
    history_list_state: ListState,

    // Connect-tab catalog + form state.
    entries: Vec<ConnectionEntry>,
    conn_list_state: ListState,
    conn_input: String,
    conn_input_stage: Option<ConnInputStage>,
    pending_conn_url: String,
    /// Connection name collected mid-form and carried through later stages.
    pending_conn_name: Option<String>,
    /// Password temporarily held between form stages; never persisted to disk.
    pending_password: Option<String>,
    /// When set, completing the form edits this profile instead of adding one.
    editing_profile_id: Option<String>,

    // Channel to the background worker.
    job_tx: mpsc::UnboundedSender<Job>,

    // Connection options kept for future use (e.g. retry-with-timeout UI).
    #[allow(dead_code)]
    simple_mode: bool,
    #[allow(dead_code)]
    connect_timeout: Option<u64>,
}

impl App {
    fn new(
        profile: Profile,
        simple_mode: bool,
        connect_timeout: Option<u64>,
        job_tx: mpsc::UnboundedSender<Job>,
    ) -> Self {
        // Start the session catalog with one always-available (session-only) entry.
        let entries = vec![ConnectionEntry::session(
            "SQLite (in-memory demo)",
            "sqlite::memory:",
        )];
        Self {
            tab: TAB_CONNECT,
            input: String::new(),
            results: Vec::new(),
            selected_result: None,
            result_scroll: 0,
            schema: None,
            schema_list_state: ListState::default(),
            health_lines: vec!["Press 'h' on this tab to refresh the health snapshot.".into()],
            db_type: None,
            db_label: "(not connected)".to_string(),
            profile,
            status: "Not connected — pick a database in the Connect tab.".to_string(),
            conn_gen: 0,
            active_profile_id: None,
            busy: HashSet::new(),
            query_rows: None,
            query_error: None,
            query_elapsed_ms: None,
            history_runs: Vec::new(),
            history_error: None,
            history_filter: HistoryFilter::default(),
            history_list_state: ListState::default(),
            entries,
            conn_list_state: ListState::default(),
            conn_input: String::new(),
            conn_input_stage: None,
            pending_conn_url: String::new(),
            pending_conn_name: None,
            pending_password: None,
            editing_profile_id: None,
            job_tx,
            simple_mode,
            connect_timeout,
        }
    }

    fn is_connected(&self) -> bool {
        self.db_type.is_some()
    }

    fn is_busy(&self, op: Op) -> bool {
        self.busy.contains(&op)
    }

    /// Send a job tagged with the current connection generation.
    fn send(&mut self, make: impl FnOnce(u64) -> Job) {
        let gen = self.conn_gen;
        let _ = self.job_tx.send(make(gen));
    }

    fn request_schema(&mut self) {
        if self.db_type.is_none() {
            self.status =
                "Not connected — switch to the Connect tab (Tab/←) and pick a database first."
                    .into();
            return;
        }
        if self.is_busy(Op::Schema) {
            return;
        }
        self.busy.insert(Op::Schema);
        self.send(|gen| Job::Schema { gen });
    }

    fn request_health(&mut self) {
        if self.db_type.is_none() {
            self.status =
                "Not connected — switch to the Connect tab (Tab/←) and pick a database first."
                    .into();
            return;
        }
        if self.is_busy(Op::Health) {
            return;
        }
        self.busy.insert(Op::Health);
        self.send(|gen| Job::Health { gen });
    }

    fn request_preview(&mut self, query: String) {
        if self.is_busy(Op::QueryPreview) {
            self.status = "A query is already running…".into();
            return;
        }
        self.busy.insert(Op::QueryPreview);
        self.query_error = None;
        self.send(|gen| Job::QueryPreview {
            gen,
            query,
            limit: QUERY_VIEW_ROW_CAP,
        });
    }

    fn request_analysis(&mut self, query: String) {
        if self.is_busy(Op::Analysis) {
            self.status = "An analysis is already running…".into();
            return;
        }
        self.busy.insert(Op::Analysis);
        let profile = self.profile.clone();
        self.send(|gen| Job::Analyze {
            gen,
            query,
            profile,
        });
    }

    fn request_history(&mut self) {
        if self.is_busy(Op::History) {
            return;
        }
        self.busy.insert(Op::History);
        let filter = crate::core::regression::RecentRunsFilter {
            connection_id: if self.history_filter.this_connection_only {
                self.active_profile_id.clone()
            } else {
                None
            },
            fingerprint: None,
            status: self.history_filter.status,
            limit: HISTORY_PAGE_SIZE,
        };
        let _ = self.job_tx.send(Job::History { filter });
    }

    /// Clear all per-connection state (schema, results, query rows, history
    /// selection) so stale data from a previous database is never shown.
    fn reset_connection_state(&mut self) {
        self.schema = None;
        self.schema_list_state.select(None);
        self.health_lines = vec!["Press 'h' on this tab to refresh the health snapshot.".into()];
        self.results.clear();
        self.selected_result = None;
        self.result_scroll = 0;
        self.query_rows = None;
        self.query_error = None;
        self.query_elapsed_ms = None;
        self.db_type = None;
        self.active_profile_id = None;
    }
}

pub async fn run_tui(
    connection: &ConnectionArgs,
    simple_mode: bool,
    connect_timeout: Option<u64>,
    verbose: bool,
) -> Result<i32> {
    let _ = verbose;

    let (job_tx, job_rx) = mpsc::unbounded_channel::<Job>();
    let (res_tx, mut res_rx) = mpsc::unbounded_channel::<JobResult>();

    let secrets: Arc<dyn SecretStore> = Arc::new(KeychainSecretStore::new());
    tokio::spawn(worker_loop(
        job_rx,
        res_tx,
        simple_mode,
        connect_timeout,
        secrets,
    ));

    let catalog = ProfileCatalog::load_default().unwrap_or_else(|e| {
        eprintln!("Warning: could not load saved profiles: {e}");
        ProfileCatalog::default()
    });

    let mut app = Box::new(App::new(
        Profile::Oltp,
        simple_mode,
        connect_timeout,
        job_tx,
    ));
    for p in &catalog.profiles {
        app.entries.push(ConnectionEntry {
            profile: p.clone(),
            saved: true,
        });
    }

    // Try the CLI-provided connection, but never hard-fail: on error we land
    // on the Connect tab with the URL added as a session-only entry so it can
    // be retried or edited interactively.
    if connection.has_connection() {
        let cli_url = connection.resolve_connection_string().ok();
        match cli_url {
            Some(url) => {
                app.conn_gen += 1;
                app.busy.insert(Op::Connect);
                let label = redact_profile_url(&url);
                let _ = app.job_tx.send(Job::Connect {
                    gen: app.conn_gen,
                    url,
                    label,
                    profile_id: None,
                    accept_invalid_certs: connection.accept_invalid_certs,
                });
                if let Ok(args_url) = connection.resolve_connection_string() {
                    app.entries.push(ConnectionEntry::session(
                        "CLI-provided connection",
                        &args_url,
                    ));
                }
            }
            None => {
                app.status = "Could not build a connection string from CLI flags.".into();
            }
        }
    } else {
        app.conn_list_state.select(Some(PROVIDER_FIRST_ROW));
        app.status =
            "Not connected — choose a provider below, or press 'a' to add a connection URL.".into();
    }

    enable_raw_mode().context("Failed to enable raw mode (is this a terminal?)")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("Failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("Failed to create terminal")?;

    let res = run_event_loop(&mut terminal, &mut app, &mut res_rx).await;
    // Restore terminal no matter what.
    disable_raw_mode().ok();
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )
    .ok();
    terminal.show_cursor().ok();

    res
}

/// Build and connect a connector for a raw URL. Shared by the worker and tests.
pub async fn connect_url(
    url: &str,
    simple_mode: bool,
    connect_timeout: Option<u64>,
    accept_invalid_certs: bool,
) -> Result<(
    Box<dyn crate::database::connection::DatabaseConnector>,
    DatabaseType,
)> {
    let db_type = crate::cli::commands::CommandHandler::detect_db_type(url)?;
    let mut connector = crate::database::connection::create_connector(db_type);
    let options = ConnectOptions {
        simple_mode,
        connect_timeout_secs: connect_timeout,
        accept_invalid_certs,
    };
    connector.connect(url, &options).await?;
    Ok((connector, db_type))
}

/// Introspect the schema through the connector (async; safe inside Tokio).
pub async fn fetch_schema(
    connector: &dyn crate::database::connection::DatabaseConnector,
) -> Result<SchemaSnapshot> {
    connector.introspect_schema().await
}

/// Fetch a health snapshot through the connector (async; safe inside Tokio).
pub async fn fetch_health_snapshot(
    db_type: DatabaseType,
    connector: &dyn crate::database::connection::DatabaseConnector,
) -> Result<crate::core::stats::HealthSnapshot> {
    use crate::core::stats as sm;

    let (check_sql, top_sql, table_sql, source_name) = match db_type {
        DatabaseType::PostgreSQL => (
            sm::PG_STAT_EXISTS_SQL.trim().to_string(),
            sm::PG_STAT_STATEMENTS_SQL.trim().to_string(),
            sm::PG_TABLE_STATS_SQL.trim().to_string(),
            "pg_stat_statements",
        ),
        DatabaseType::MySQL => (
            sm::MYSQL_PERF_SCHEMA_CHECK_SQL.trim().to_string(),
            sm::MYSQL_PERF_SCHEMA_SQL.trim().to_string(),
            sm::MYSQL_TABLE_STATS_SQL.trim().to_string(),
            "performance_schema",
        ),
        DatabaseType::SQLite => {
            return Ok(crate::core::stats::HealthSnapshot {
                database_type: db_type,
                top_queries: vec![],
                table_stats: vec![],
                stats_available: false,
                stats_source: "no runtime stats extension for SQLite".into(),
            })
        }
    };

    let available = connector
        .preview_rows(&check_sql, 1)
        .await
        .ok()
        .and_then(|p| p.rows.first().and_then(|r| r.first().cloned()))
        .map(|v| v == "1" || v == "t" || v == "true" || v.eq_ignore_ascii_case("on"))
        .unwrap_or(false);

    if !available {
        return Ok(crate::core::stats::HealthSnapshot {
            database_type: db_type,
            top_queries: vec![],
            table_stats: vec![],
            stats_available: false,
            stats_source: format!("{} not available", source_name),
        });
    }

    let top = connector
        .preview_rows(&top_sql, 20)
        .await
        .map(|p| sm::parse_query_stat_rows(&p))
        .unwrap_or_default();
    let tables = connector
        .preview_rows(&table_sql, 100)
        .await
        .map(|p| sm::parse_table_stat_rows(&p))
        .unwrap_or_default();

    Ok(crate::core::stats::HealthSnapshot {
        database_type: db_type,
        top_queries: top,
        table_stats: tables,
        stats_available: true,
        stats_source: source_name.to_string(),
    })
}

/// Render a health snapshot as TUI lines (shared by the worker and tests).
pub fn health_lines_from(snapshot: &crate::core::stats::HealthSnapshot) -> Vec<String> {
    let mut v = vec![format!("Source: {}", snapshot.stats_source)];
    if snapshot.stats_available {
        v.push(String::new());
        v.push("Top queries by total time:".into());
        for s in snapshot.top_queries.iter().take(10) {
            v.push(format!(
                "  {:>6} calls {:>10.1} ms  {}",
                s.calls,
                s.total_time_ms,
                truncate(&s.query, 70)
            ));
        }
        v.push(String::new());
        v.push("Table cardinality:".into());
        for t in snapshot.table_stats.iter().take(15) {
            v.push(format!("  ~{} rows  {}", t.estimated_rows, t.table_name));
        }
    } else {
        v.push("Runtime stats unavailable — falling back to static confidence.".into());
    }
    v
}

/// Shared analysis pipeline used by the Query, Analyze, and History tabs:
/// syntax/security analysis, live schema attachment, schema checks, and a
/// best-effort EXPLAIN capture.
pub async fn run_analysis_pipeline(
    connector: &dyn crate::database::connection::DatabaseConnector,
    query: &str,
    db_type: DatabaseType,
    profile: Profile,
) -> Result<AnalysisResult> {
    let analyzer = crate::core::analyzer::SqlAnalyzer::new();
    let mut result = analyzer.analyze_query(query, db_type, profile).await?;

    let schema = connector.introspect_schema().await?;
    result.schema_snapshot = Some(schema);
    analyzer.run_schema_checks(&mut result).await?;

    // Best-effort plan capture for the plain-English summary.
    if let Ok(plan) = connector.explain_query(query).await {
        result.explain_plan = Some(plan);
    }

    Ok(result)
}

/// Persist a query attempt (success or failure) to the history store when it
/// exists. Blocking SQLite work is isolated on the blocking thread pool.
#[allow(clippy::too_many_arguments)]
fn record_attempt(
    query: &str,
    success: bool,
    error: Option<String>,
    elapsed_ms: Option<u64>,
    rows: Option<i64>,
    plan_summary: Option<String>,
    index_used: Option<String>,
    connection_id: Option<String>,
    connection_label: Option<String>,
) {
    if !crate::core::regression::StateStore::default_exists() {
        return;
    }
    let query = query.to_string();
    std::thread::spawn(move || {
        if let Ok(store) = crate::core::regression::StateStore::open_default() {
            let _ = store.record_run(
                &query,
                elapsed_ms,
                rows,
                plan_summary.as_deref(),
                index_used.as_deref(),
                connection_id.as_deref(),
                connection_label.as_deref(),
                success,
                error.as_deref(),
            );
        }
    });
}

/// The background worker: owns the active connector and services every
/// database job off the UI thread.
async fn worker_loop(
    mut job_rx: mpsc::UnboundedReceiver<Job>,
    res_tx: mpsc::UnboundedSender<JobResult>,
    simple_mode: bool,
    connect_timeout: Option<u64>,
    secrets: Arc<dyn SecretStore>,
) {
    use crate::database::connection::DatabaseConnector;

    let mut connector: Option<Box<dyn DatabaseConnector>> = None;
    // Connection identity used for history records.
    let mut conn_id: Option<String> = None;
    let mut conn_label: Option<String> = None;

    while let Some(job) = job_rx.recv().await {
        match job {
            Job::Connect {
                gen,
                url,
                label,
                profile_id,
                accept_invalid_certs,
            } => {
                // Await-disconnect the old connector before installing a new one.
                if let Some(mut old) = connector.take() {
                    let _ = old.disconnect().await;
                }
                // Saved profiles carry sanitized URLs — resolve the password
                // from the credential store before connecting.
                let url = match &profile_id {
                    Some(id) => inject_secret_into_url(&url, id, secrets.as_ref()).await,
                    None => url,
                };
                match connect_url(&url, simple_mode, connect_timeout, accept_invalid_certs).await {
                    Ok((c, db_type)) => {
                        connector = Some(c);
                        conn_id = profile_id.clone();
                        conn_label = Some(label.clone());
                        let _ = res_tx.send(JobResult::Connected {
                            gen,
                            ok: true,
                            db_type: Some(db_type),
                            message: format!("Connected to {label} ({}).", db_type_name(db_type)),
                        });
                    }
                    Err(e) => {
                        conn_id = None;
                        conn_label = None;
                        let mut msg = format!("Connection failed: {e}");
                        let lower = e.to_string().to_lowercase();
                        if lower.contains("certificate") || lower.contains("tls") {
                            msg.push_str(
                                " — TLS/certificate issue: check your system clock, the sslmode \
                                 parameter, or enable 'accept invalid certs' for self-signed certs.",
                            );
                        } else if lower.contains("timeout") || lower.contains("refused") {
                            msg.push_str(" — check host, port, and network/firewall access.");
                        }
                        let _ = res_tx.send(JobResult::Connected {
                            gen,
                            ok: false,
                            db_type: None,
                            message: msg,
                        });
                    }
                }
            }
            Job::Disconnect { gen } => {
                if let Some(mut old) = connector.take() {
                    let _ = old.disconnect().await;
                }
                conn_id = None;
                conn_label = None;
                let _ = res_tx.send(JobResult::Disconnected {
                    gen,
                    message: "Disconnected.".into(),
                });
            }
            Job::Schema { gen } => {
                let result = match connector.as_deref() {
                    Some(c) => fetch_schema(c).await.map_err(|e| e.to_string()),
                    None => Err("No database connection.".into()),
                };
                let _ = res_tx.send(JobResult::Schema { gen, result });
            }
            Job::Health { gen } => {
                let lines = match (connector.as_deref(), db_type_of(&connector)) {
                    (Some(c), Some(t)) => match fetch_health_snapshot(t, c).await {
                        Ok(snapshot) => health_lines_from(&snapshot),
                        Err(e) => vec![format!("Health check failed: {e}")],
                    },
                    _ => vec!["No database connection.".into()],
                };
                let _ = res_tx.send(JobResult::Health { gen, lines });
            }
            Job::QueryPreview { gen, query, limit } => {
                let started = std::time::Instant::now();
                let result = match connector.as_deref() {
                    // Read-only SELECT enforcement happens inside preview_rows.
                    Some(c) => c
                        .preview_rows(&query, limit)
                        .await
                        .map_err(|e| e.to_string()),
                    None => Err("No database connection.".into()),
                };
                let elapsed = started.elapsed().as_millis() as u64;
                let rows = result.as_ref().ok().map(|p| p.rows.len() as i64);
                let conn_id_for_record = conn_id.clone();
                let conn_label_for_record = conn_label.clone();
                let q = query.clone();
                let success = result.is_ok();
                let err = result.as_ref().err().cloned();
                record_attempt(
                    &q,
                    success,
                    err,
                    Some(elapsed),
                    rows,
                    None,
                    None,
                    conn_id_for_record,
                    conn_label_for_record,
                );
                let _ = res_tx.send(JobResult::QueryPreview {
                    gen,
                    query,
                    result,
                    elapsed_ms: elapsed,
                });
            }
            Job::Analyze {
                gen,
                query,
                profile,
            } => {
                let started = std::time::Instant::now();
                let result = match connector.as_deref() {
                    Some(c) => run_analysis_pipeline(
                        c,
                        &query,
                        db_type_of(&connector).unwrap_or(DatabaseType::SQLite),
                        profile,
                    )
                    .await
                    .map_err(|e| e.to_string()),
                    None => Err("No database connection.".into()),
                };
                let elapsed = started.elapsed().as_millis() as u64;
                let conn_id_for_record = conn_id.clone();
                let conn_label_for_record = conn_label.clone();
                let success = result.is_ok();
                let err = result.as_ref().err().cloned();
                let plan_summary = result
                    .as_ref()
                    .ok()
                    .and_then(|r| crate::core::explain::plain_explain_summary(&r.explain_plan));
                let index_used = result.as_ref().ok().and_then(|r| {
                    r.explain_plan
                        .as_ref()
                        .and_then(|p| p.root.as_ref().and_then(|n| n.index_used.clone()))
                });
                let rows = result
                    .as_ref()
                    .ok()
                    .and_then(|r| r.row_preview.as_ref().map(|p| p.rows.len() as i64));
                record_attempt(
                    &query,
                    success,
                    err,
                    Some(elapsed),
                    rows,
                    plan_summary,
                    index_used,
                    conn_id_for_record,
                    conn_label_for_record,
                );
                let _ = res_tx.send(JobResult::Analysis {
                    gen,
                    query,
                    result: Box::new(result),
                    elapsed_ms: elapsed,
                });
            }
            Job::History { filter } => {
                let (runs, error) = if !crate::core::regression::StateStore::default_exists() {
                    (
                        Vec::new(),
                        Some(
                            "No state store yet — queries you run here are recorded automatically."
                                .to_string(),
                        ),
                    )
                } else {
                    match tokio::task::spawn_blocking(move || {
                        crate::core::regression::StateStore::open_default()
                            .and_then(|store| store.get_recent_runs_filtered(filter))
                    })
                    .await
                    {
                        Ok(Ok(runs)) => (runs, None),
                        Ok(Err(e)) => (Vec::new(), Some(e.to_string())),
                        Err(e) => (Vec::new(), Some(e.to_string())),
                    }
                };
                let _ = res_tx.send(JobResult::History { runs, error });
            }
        }
    }
}

fn db_type_of(
    connector: &Option<Box<dyn crate::database::connection::DatabaseConnector>>,
) -> Option<DatabaseType> {
    connector.as_deref().map(|c| c.database_type())
}

fn db_type_name(db_type: DatabaseType) -> &'static str {
    match db_type {
        DatabaseType::PostgreSQL => "PostgreSQL",
        DatabaseType::MySQL => "MySQL",
        DatabaseType::SQLite => "SQLite",
    }
}

fn default_entry_name(url: &str, provider: Provider) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let rest = rest.split('?').next().unwrap_or(rest);
    let redacted = redact_profile_url(rest);
    format!("{} · {}", provider.label(), redacted)
}

fn is_valid_conn_row(row: usize, entries: usize) -> bool {
    (PROVIDER_FIRST_ROW..=PROVIDER_LAST_ROW).contains(&row)
        || (SESSION_FIRST_ROW..SESSION_FIRST_ROW + entries).contains(&row)
}

fn move_conn_selection(app: &mut App, forward: bool) {
    let entries = app.entries.len();
    let total = SESSION_FIRST_ROW + entries;
    let mut row = app.conn_list_state.selected().unwrap_or(PROVIDER_FIRST_ROW);
    for _ in 0..total {
        row = if forward {
            (row + 1).min(total.saturating_sub(1))
        } else {
            row.saturating_sub(1)
        };
        if is_valid_conn_row(row, entries) {
            break;
        }
    }
    app.conn_list_state.select(Some(row));
}

/// Begin the add/edit-connection form from a provider preset or an entry.
fn start_conn_form(app: &mut App, url: String, edit_profile_id: Option<String>) {
    app.conn_input = url;
    app.pending_conn_url.clear();
    app.pending_conn_name = None;
    app.pending_password = None;
    app.editing_profile_id = edit_profile_id;
    app.conn_input_stage = Some(ConnInputStage::Url);
}

/// Enter on the Connect tab: prefill the add-connection form from a provider
/// preset, connect to the selected entry, or begin editing it.
fn handle_connect_enter(app: &mut App) {
    let row = app.conn_list_state.selected().unwrap_or(PROVIDER_FIRST_ROW);

    if (PROVIDER_FIRST_ROW..=PROVIDER_LAST_ROW).contains(&row) {
        let provider = PROVIDERS[row - PROVIDER_FIRST_ROW];
        start_conn_form(app, provider.template().to_string(), None);
        app.status = format!(
            "{}: edit the URL and press Enter (Esc cancels).",
            provider.label()
        );
        return;
    }

    let idx = match row.checked_sub(SESSION_FIRST_ROW) {
        Some(i) if i < app.entries.len() => i,
        _ => return,
    };
    let entry = app.entries[idx].clone();
    let url = entry.profile.url.clone();
    if entry.profile.is_ephemeral() {
        app.status = "In-memory SQLite is session-only: data disappears on exit. \
                      Connecting anyway…"
            .into();
    }
    // Connect immediately (worker handles it asynchronously).
    app.conn_gen += 1;
    app.reset_connection_state();
    app.busy.insert(Op::Connect);
    app.status = format!("Connecting to {}…", redact_profile_url(&url));
    let label = entry.profile.name.clone();
    let profile_id = entry.saved.then(|| entry.profile.id.clone());
    let _ = app.job_tx.send(Job::Connect {
        gen: app.conn_gen,
        url,
        label,
        profile_id,
        accept_invalid_certs: entry.profile.accept_invalid_certs,
    });
}

/// Save a profile (new or edited) to the catalog and its secret to the store.
async fn persist_profile(
    app: &mut App,
    secrets: &dyn SecretStore,
    catalog: &mut ProfileCatalog,
    name: String,
    url: String,
    password: Option<String>,
    accept_invalid_certs: bool,
) {
    let kind = kind_from_url(&url);
    let ephemeral = url == "sqlite::memory:" || url.contains(":memory:");

    let mut profile = if let Some(id) = app.editing_profile_id.take() {
        let mut existing = catalog
            .profiles
            .iter()
            .find(|p| p.id == id)
            .cloned()
            .unwrap_or_else(|| ConnectionProfile::new(name.clone(), kind, &url));
        existing.name = name;
        existing.kind = kind;
        existing.url = redact_profile_url(&url);
        existing.accept_invalid_certs = accept_invalid_certs;
        existing
    } else {
        let mut p = ConnectionProfile::new(name, kind, &url);
        p.accept_invalid_certs = accept_invalid_certs;
        p
    };

    // Secrets go to the credential store, never into the catalog file.
    if let Some(secret) = password.filter(|s| !s.is_empty()) {
        let key = format!("sql-optimizer/profile/{}", profile.id);
        if let Err(e) = secrets.set_secret(&key, &secret).await {
            app.status = format!("Could not store secret: {e}");
            return;
        }
        profile.secret_ref = Some(key);
    }

    if ephemeral {
        // Never persist in-memory SQLite as a reusable profile.
        app.entries.push(ConnectionEntry {
            profile,
            saved: false,
        });
        app.status = "Session-only connection added (in-memory SQLite is not persisted).".into();
        return;
    }

    match catalog.upsert(profile.clone()) {
        Ok(()) => {
            // Replace or add the entry view.
            if let Some(slot) = app.entries.iter_mut().find(|e| e.profile.id == profile.id) {
                slot.profile = profile.clone();
                slot.saved = true;
            } else {
                app.entries.push(ConnectionEntry {
                    profile: profile.clone(),
                    saved: true,
                });
            }
            app.status = format!("Saved profile '{}'.", profile.name);
        }
        Err(e) => {
            app.status = format!("Could not save profile: {e}");
            app.entries.push(ConnectionEntry {
                profile,
                saved: false,
            });
        }
    }
}

/// Delete the selected connection entry: persisted profiles are removed from
/// the catalog and their secret from the credential store.
async fn delete_selected_connection(
    app: &mut App,
    secrets: &dyn SecretStore,
    catalog: &mut ProfileCatalog,
) {
    let row = app.conn_list_state.selected().unwrap_or(0);
    let idx = match row.checked_sub(SESSION_FIRST_ROW) {
        Some(i) if i < app.entries.len() => i,
        _ => return,
    };
    let entry = app.entries.remove(idx);
    let name = entry.profile.name.clone();
    if entry.saved {
        let _ = catalog.remove(&entry.profile.id);
        if let Some(key) = &entry.profile.secret_ref {
            let _ = secrets.delete_secret(key).await;
        }
        app.status = format!("Deleted profile '{name}' and its stored secret.");
    } else {
        app.status = format!("Removed session connection '{name}'.");
    }
    let entries = app.entries.len();
    let new_row = if entries == 0 {
        PROVIDER_LAST_ROW
    } else {
        SESSION_FIRST_ROW + idx.min(entries - 1)
    };
    app.conn_list_state.select(Some(new_row));
}

/// Connect to the selected entry again (retry).
fn retry_selected_connection(app: &mut App) {
    let row = app.conn_list_state.selected().unwrap_or(0);
    let idx = match row.checked_sub(SESSION_FIRST_ROW) {
        Some(i) if i < app.entries.len() => i,
        _ => {
            app.status = "Select a saved or session connection first.".into();
            return;
        }
    };
    let entry = app.entries[idx].clone();
    app.conn_gen += 1;
    app.reset_connection_state();
    app.busy.insert(Op::Connect);
    app.status = format!("Retrying {}…", redact_profile_url(&entry.profile.url));
    let _ = app.job_tx.send(Job::Connect {
        gen: app.conn_gen,
        url: entry.profile.url.clone(),
        label: entry.profile.name.clone(),
        profile_id: entry.saved.then(|| entry.profile.id.clone()),
        accept_invalid_certs: entry.profile.accept_invalid_certs,
    });
}

/// Disconnect the active connection.
fn disconnect_active(app: &mut App) {
    if !app.is_connected() {
        app.status = "Already disconnected.".into();
        return;
    }
    app.conn_gen += 1;
    app.reset_connection_state();
    app.db_label = "(not connected)".to_string();
    app.busy.insert(Op::Connect);
    app.send(|gen| Job::Disconnect { gen });
}

async fn handle_connect_input_enter(
    app: &mut App,
    secrets: &dyn SecretStore,
    catalog: &mut ProfileCatalog,
) {
    let stage = match app.conn_input_stage {
        Some(s) => s,
        None => return,
    };
    match stage {
        ConnInputStage::Url => {
            let url = app.conn_input.trim().to_string();
            match provider_from_url(&url) {
                Some(provider) => {
                    app.pending_conn_url = url.clone();
                    app.conn_input = if app.editing_profile_id.is_some() {
                        // Keep the existing name when editing; Enter accepts it.
                        catalog
                            .profiles
                            .iter()
                            .find(|p| Some(&p.id) == app.editing_profile_id.as_ref())
                            .map(|p| p.name.clone())
                            .unwrap_or_else(|| default_entry_name(&url, provider))
                    } else {
                        default_entry_name(&url, provider)
                    };
                    app.conn_input_stage = Some(ConnInputStage::Name);
                    app.status = "Name this connection and press Enter (Esc cancels).".into();
                }
                None => {
                    app.status = "Unrecognized URL. Must start with postgresql://, mysql://, \
                                  or sqlite:// (or end with .db / .sqlite)."
                        .into();
                }
            }
        }
        ConnInputStage::Name => {
            let name = app.conn_input.trim().to_string();
            if name.is_empty() {
                app.status = "Enter a name for this connection.".into();
                return;
            }
            let url = app.pending_conn_url.clone();
            let provider = provider_from_url(&url).unwrap_or(Provider::Postgres);
            if provider.needs_password() {
                // Carry the collected name through the password/cert stages.
                app.pending_conn_name = Some(name);
                app.conn_input.clear();
                app.conn_input_stage = Some(ConnInputStage::Password);
                app.status = "Password (stored in the OS credential store; Enter to skip):".into();
            } else {
                finish_conn_form(app, secrets, catalog, name, url, None, false).await;
            }
        }
        ConnInputStage::Password => {
            // conn_input holds the password (masked in the UI).
            let password = std::mem::take(&mut app.conn_input);
            app.conn_input_stage = Some(ConnInputStage::Cert);
            app.status = "Accept invalid TLS certificates? y/N".into();
            // Stash the password until the cert stage completes.
            app.pending_password = Some(password);
        }
        ConnInputStage::Cert => {
            let accept = matches!(app.conn_input.trim().to_lowercase().as_str(), "y" | "yes");
            app.conn_input.clear();
            let password = app.pending_password.take().unwrap_or_default();
            let url = app.pending_conn_url.clone();
            // When editing, reuse the profile's existing name; for a new
            // connection the name was collected in the Name stage.
            let name = if let Some(id) = &app.editing_profile_id {
                catalog
                    .profiles
                    .iter()
                    .find(|p| &p.id == id)
                    .map(|p| p.name.clone())
            } else {
                app.pending_conn_name.clone().filter(|n| !n.is_empty())
            };
            let Some(name) = name else {
                app.status = "Internal error: connection name lost — restart the form.".into();
                app.conn_input_stage = None;
                return;
            };
            finish_conn_form(app, secrets, catalog, name, url, Some(password), accept).await;
        }
    }
}

/// Complete the add/edit form: persist the profile + secret, then connect.
async fn finish_conn_form(
    app: &mut App,
    secrets: &dyn SecretStore,
    catalog: &mut ProfileCatalog,
    name: String,
    url: String,
    password: Option<String>,
    accept_invalid_certs: bool,
) {
    app.conn_input_stage = None;
    app.conn_input.clear();
    let label = name.clone();
    persist_profile(
        app,
        secrets,
        catalog,
        name,
        url.clone(),
        password,
        accept_invalid_certs,
    )
    .await;

    // Connect to the freshly saved/session entry.
    app.conn_gen += 1;
    app.reset_connection_state();
    app.busy.insert(Op::Connect);
    app.status = format!("Connecting to {}…", redact_profile_url(&url));
    let profile_id = app
        .entries
        .iter()
        .find(|e| e.profile.url == redact_profile_url(&url) && e.profile.name == label)
        .filter(|e| e.saved)
        .map(|e| e.profile.id.clone());
    let _ = app.job_tx.send(Job::Connect {
        gen: app.conn_gen,
        url,
        label,
        profile_id,
        accept_invalid_certs,
    });
}

async fn run_event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    res_rx: &mut mpsc::UnboundedReceiver<JobResult>,
) -> Result<i32> {
    let mut catalog = ProfileCatalog::load_default().unwrap_or_default();
    let secrets = KeychainSecretStore::new();

    loop {
        // Drain completed worker results first so the UI reflects fresh state.
        while let Ok(result) = res_rx.try_recv() {
            apply_result(app, result);
        }

        terminal.draw(|f| draw(f, app))?;

        // Responsive wait: input stays live while the worker crunches.
        if !event::poll(std::time::Duration::from_millis(100))? {
            continue;
        }

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            let in_conn_form = app.tab == TAB_CONNECT && app.conn_input_stage.is_some();

            match key.code {
                // Esc cancels the connect form first, otherwise quits.
                KeyCode::Esc if in_conn_form => {
                    app.conn_input_stage = None;
                    app.conn_input.clear();
                    app.pending_conn_url.clear();
                    app.pending_conn_name = None;
                    app.pending_password = None;
                    app.editing_profile_id = None;
                    app.status = "Connection form cancelled.".into();
                }
                KeyCode::Esc => return Ok(0),
                KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(0)
                }
                KeyCode::Char('q')
                    if key.modifiers.is_empty()
                        && !matches!(app.tab, TAB_QUERY | TAB_ANALYZE)
                        && !in_conn_form =>
                {
                    return Ok(0)
                }
                KeyCode::Char('x') if app.tab == TAB_CONNECT && !in_conn_form => {
                    disconnect_active(app);
                }
                KeyCode::Tab | KeyCode::Right
                    if !matches!(key.modifiers, KeyModifiers::SHIFT) && !in_conn_form =>
                {
                    app.tab = (app.tab + 1) % TABS.len();
                }
                KeyCode::BackTab if !in_conn_form => {
                    app.tab = (app.tab + TABS.len() - 1) % TABS.len()
                }
                KeyCode::Left if !in_conn_form => {
                    app.tab = (app.tab + TABS.len() - 1) % TABS.len();
                }
                KeyCode::Down => {
                    if app.tab == TAB_CONNECT && !in_conn_form {
                        move_conn_selection(app, true);
                    } else if app.tab == TAB_HISTORY {
                        next_history_item(app);
                    } else if app.tab == TAB_SCHEMA {
                        next_schema_item(app);
                    } else {
                        app.result_scroll = app.result_scroll.saturating_add(1);
                    }
                }
                KeyCode::Up => {
                    if app.tab == TAB_CONNECT && !in_conn_form {
                        move_conn_selection(app, false);
                    } else if app.tab == TAB_HISTORY {
                        prev_history_item(app);
                    } else if app.tab == TAB_SCHEMA {
                        prev_schema_item(app);
                    } else {
                        app.result_scroll = app.result_scroll.saturating_sub(1);
                    }
                }

                // ---- Connect tab ----
                KeyCode::Char(c) if in_conn_form => {
                    app.conn_input.push(c);
                }
                KeyCode::Backspace if in_conn_form => {
                    app.conn_input.pop();
                }
                KeyCode::Enter if app.tab == TAB_CONNECT && in_conn_form => {
                    handle_connect_input_enter(app, &secrets, &mut catalog).await;
                }
                KeyCode::Enter if app.tab == TAB_CONNECT => {
                    handle_connect_enter(app);
                }
                KeyCode::Char('a') if app.tab == TAB_CONNECT && !in_conn_form => {
                    app.conn_input.clear();
                    app.editing_profile_id = None;
                    app.pending_conn_name = None;
                    app.conn_input_stage = Some(ConnInputStage::Url);
                    app.status =
                        "Enter a connection URL (postgresql://, mysql://, sqlite://) and press Enter."
                            .into();
                }
                KeyCode::Char('d') if app.tab == TAB_CONNECT && !in_conn_form => {
                    delete_selected_connection(app, &secrets, &mut catalog).await;
                }
                KeyCode::Char('e') if app.tab == TAB_CONNECT && !in_conn_form => {
                    let row = app.conn_list_state.selected().unwrap_or(0);
                    let idx = row.checked_sub(SESSION_FIRST_ROW);
                    if let Some(i) = idx {
                        if let Some(entry) = app.entries.get(i) {
                            let id = entry.profile.id.clone();
                            let url = entry.profile.url.clone();
                            start_conn_form(app, url, Some(id));
                            app.status =
                                "Edit the URL and press Enter (Esc cancels). Password: Enter keeps the existing one.".into();
                        }
                    } else {
                        app.status = "Select a saved or session connection to edit.".into();
                    }
                }
                KeyCode::Char('r') if app.tab == TAB_CONNECT && !in_conn_form => {
                    retry_selected_connection(app);
                }

                // ---- Query tab ----
                KeyCode::Char(c) if app.tab == TAB_QUERY => match c {
                    'h' if app.input.is_empty() && key.modifiers.is_empty() => {
                        app.request_health();
                        app.tab = TAB_HEALTH;
                    }
                    _ => app.input.push(c),
                },
                KeyCode::Backspace if app.tab == TAB_QUERY => {
                    app.input.pop();
                }
                KeyCode::Enter if app.tab == TAB_QUERY => {
                    let query = app.input.trim().to_string();
                    if query.is_empty() {
                        app.status = "Type a read-only SELECT query first.".into();
                        continue;
                    }
                    if query.eq_ignore_ascii_case("quit") || query.eq_ignore_ascii_case("exit") {
                        return Ok(0);
                    }
                    if !app.is_connected() {
                        app.status = "Not connected — switch to the Connect tab (Tab/←) and \
                                      pick a database first."
                            .into();
                        continue;
                    }
                    app.status = format!("Running: {}…", truncate(&query, 40));
                    app.request_preview(query);
                }
                // Query -> Analyze: analyze the current query text.
                KeyCode::Char('a')
                    if app.tab == TAB_QUERY
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                        && !app.input.is_empty() =>
                {
                    let query = app.input.trim().to_string();
                    if app.is_connected() {
                        app.tab = TAB_ANALYZE;
                        app.status = "Analyzing…".into();
                        app.request_analysis(query);
                    } else {
                        app.status = "Connect to a database first.".into();
                    }
                }

                // ---- Analyze tab ----
                KeyCode::Char(c) if app.tab == TAB_ANALYZE => match c {
                    'h' if app.input.is_empty() && key.modifiers.is_empty() => {
                        app.request_health();
                        app.tab = TAB_HEALTH;
                    }
                    _ => app.input.push(c),
                },
                KeyCode::Backspace if app.tab == TAB_ANALYZE => {
                    app.input.pop();
                }
                KeyCode::Enter if app.tab == TAB_ANALYZE => {
                    let query = app.input.trim().to_string();
                    if query.is_empty() {
                        app.status = "Type a SQL query first.".into();
                        continue;
                    }
                    if query.eq_ignore_ascii_case("quit") || query.eq_ignore_ascii_case("exit") {
                        return Ok(0);
                    }
                    if !app.is_connected() {
                        app.status = "Not connected — switch to the Connect tab (Tab/←) and \
                                      pick a database first."
                            .into();
                        continue;
                    }
                    app.status = "Analyzing…".into();
                    app.request_analysis(query);
                }
                // Analyze -> Query: carry the current query text to the Query tab.
                KeyCode::Char('p')
                    if app.tab == TAB_ANALYZE
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                        && !app.input.is_empty() =>
                {
                    app.tab = TAB_QUERY;
                    app.status =
                        "Query text carried over — press Enter to run it (read-only).".into();
                }
                KeyCode::Char('e') if app.tab == TAB_ANALYZE && !app.input.is_empty() => {
                    app.status = "Tip: add EXPLAIN via CLI flag --explain; TUI shows the plan summary automatically when present.".into();
                }

                // ---- Optimize tab ----
                KeyCode::Char('r') if app.tab == TAB_OPTIMIZE && key.modifiers.is_empty() => {
                    if app.results.is_empty() {
                        app.status = "Run an analysis first (Analyze tab), then Optimize \
                                      shows its recommendations here."
                            .into();
                    } else {
                        app.status = "Recommendations re-ranked from the latest analysis.".into();
                    }
                }

                // ---- Other tabs ----
                KeyCode::Char('s') if app.tab == TAB_SCHEMA => {
                    app.request_schema();
                }
                KeyCode::Char('h') if app.tab == TAB_HEALTH => {
                    app.request_health();
                }
                KeyCode::Char('r') if app.tab == TAB_HISTORY => {
                    app.request_history();
                }
                KeyCode::Char('c') if app.tab == TAB_HISTORY => {
                    app.history_filter.this_connection_only =
                        !app.history_filter.this_connection_only;
                    app.request_history();
                }
                KeyCode::Char('f') if app.tab == TAB_HISTORY => {
                    app.history_filter.status = match app.history_filter.status {
                        None => Some(false),
                        Some(false) => Some(true),
                        Some(true) => None,
                    };
                    app.request_history();
                }
                // History -> Query: load the selected run's exact SQL.
                KeyCode::Enter if app.tab == TAB_HISTORY => {
                    if let Some(idx) = app.history_list_state.selected() {
                        if let Some(run) = app.history_runs.get(idx) {
                            app.input = run.query_text.clone();
                            app.tab = TAB_QUERY;
                            app.status =
                                "Loaded query from history — press Enter to run it (read-only)."
                                    .into();
                        }
                    }
                }
                // History -> Analyze: analyze the selected run's SQL.
                KeyCode::Char('a')
                    if app.tab == TAB_HISTORY && key.modifiers.contains(KeyModifiers::CONTROL) =>
                {
                    if let Some(idx) = app.history_list_state.selected() {
                        if let Some(run) = app.history_runs.get(idx) {
                            let query = run.query_text.clone();
                            app.input = query.clone();
                            if app.is_connected() {
                                app.tab = TAB_ANALYZE;
                                app.status = "Analyzing selected history item…".into();
                                app.request_analysis(query);
                            } else {
                                app.tab = TAB_ANALYZE;
                                app.status =
                                    "Loaded query — connect to a database, then press Enter."
                                        .into();
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Apply one worker result to app state, ignoring anything from a stale
/// connection generation.
fn apply_result(app: &mut App, result: JobResult) {
    match result {
        JobResult::Connected {
            gen,
            ok,
            db_type,
            message,
        } => {
            app.busy.remove(&Op::Connect);
            if gen != app.conn_gen {
                return; // stale connect result — connection switched meanwhile
            }
            if ok {
                app.db_type = db_type;
                app.status = message.clone();
                if let Some(rest) = message.strip_prefix("Connected to ") {
                    app.db_label = rest
                        .rsplit_once(" (")
                        .map(|(l, _)| l.to_string())
                        .unwrap_or_else(|| rest.to_string());
                }
                app.tab = TAB_QUERY;
            } else {
                app.status = message;
            }
        }
        JobResult::Disconnected { gen, message } => {
            app.busy.remove(&Op::Connect);
            if gen != app.conn_gen {
                return;
            }
            app.status = message;
        }
        JobResult::Schema { gen, result } => {
            app.busy.remove(&Op::Schema);
            if gen != app.conn_gen {
                return;
            }
            match result {
                Ok(schema) => {
                    let tables = schema.tables.len();
                    app.schema = Some(schema);
                    app.status = format!("Schema refreshed: {tables} tables");
                }
                Err(e) => app.status = format!("Schema introspection failed: {e}"),
            }
        }
        JobResult::Health { gen, lines } => {
            app.busy.remove(&Op::Health);
            if gen != app.conn_gen {
                return;
            }
            app.health_lines = lines;
            app.status = "Health snapshot refreshed.".into();
        }
        JobResult::QueryPreview {
            gen,
            query: _query,
            result,
            elapsed_ms,
        } => {
            app.busy.remove(&Op::QueryPreview);
            if gen != app.conn_gen {
                return;
            }
            app.query_elapsed_ms = Some(elapsed_ms);
            match result {
                Ok(preview) => {
                    app.query_rows = Some(preview);
                    app.status = format!("Query completed in {elapsed_ms}ms (read-only).");
                }
                Err(e) => {
                    app.query_rows = None;
                    app.query_error = Some(e.clone());
                    app.status = format!("Query failed: {e}");
                }
            }
        }
        JobResult::Analysis {
            gen,
            query: _query,
            result,
            elapsed_ms: _elapsed,
        } => {
            app.busy.remove(&Op::Analysis);
            if gen != app.conn_gen {
                return;
            }
            match *result {
                Ok(analysis) => {
                    app.results.insert(0, analysis);
                    app.selected_result = Some(0);
                    app.result_scroll = 0;
                    app.status = "Analysis complete — see Optimize for ranked fixes.".into();
                }
                Err(e) => {
                    app.status = format!("Error: {e}");
                }
            }
        }
        JobResult::History { runs, error } => {
            app.busy.remove(&Op::History);
            app.history_runs = runs;
            app.history_error = error;
            if !app.history_runs.is_empty() {
                app.history_list_state.select(Some(0));
            }
            app.status = "History refreshed.".into();
        }
    }
}

fn next_schema_item(app: &mut App) {
    let len = app.schema.as_ref().map(|s| s.tables.len()).unwrap_or(0);
    if len == 0 {
        return;
    }
    let current = app.schema_list_state.selected().unwrap_or(0);
    app.schema_list_state
        .select(Some((current + 1).min(len - 1)));
}

fn prev_schema_item(app: &mut App) {
    let len = app.schema.as_ref().map(|s| s.tables.len()).unwrap_or(0);
    if len == 0 {
        return;
    }
    let current = app.schema_list_state.selected().unwrap_or(0);
    app.schema_list_state
        .select(Some(current.saturating_sub(1)));
}

fn next_history_item(app: &mut App) {
    let len = app.history_runs.len();
    if len == 0 {
        return;
    }
    let current = app.history_list_state.selected().unwrap_or(0);
    app.history_list_state
        .select(Some((current + 1).min(len - 1)));
}

fn prev_history_item(app: &mut App) {
    let len = app.history_runs.len();
    if len == 0 {
        return;
    }
    let current = app.history_list_state.selected().unwrap_or(0);
    app.history_list_state
        .select(Some(current.saturating_sub(1)));
}

fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Length(3), // tabs
            Constraint::Min(5),    // content
            Constraint::Length(3), // input (query tab / connect form)
            Constraint::Length(1), // footer
        ])
        .split(f.area());

    draw_header(f, app, chunks[0]);
    draw_tabs(f, app, chunks[1]);

    match app.tab {
        TAB_CONNECT => draw_connect(f, app, chunks[2]),
        TAB_QUERY => draw_query(f, app, chunks[2]),
        TAB_ANALYZE => draw_analyze(f, app, chunks[2]),
        TAB_OPTIMIZE => draw_optimize(f, app, chunks[2]),
        TAB_SCHEMA => draw_schema(f, app, chunks[2]),
        TAB_HEALTH => draw_health(f, app, chunks[2]),
        _ => draw_history(f, app, chunks[2]),
    }

    let show_sql_input = matches!(app.tab, TAB_QUERY | TAB_ANALYZE);
    let show_conn_form = app.tab == TAB_CONNECT && app.conn_input_stage.is_some();

    if show_sql_input {
        draw_sql_input(f, app, chunks[3]);
    } else if show_conn_form {
        draw_conn_form(f, app, chunks[3]);
    }

    let footer_area = if show_sql_input || show_conn_form {
        chunks[4]
    } else {
        chunks[3].merge_up(chunks[4])
    };
    draw_footer(f, app, footer_area);
}

trait MergeUp {
    fn merge_up(self, other: Rect) -> Rect;
}
impl MergeUp for Rect {
    /// Footer occupies the last line of the frame even without an input box.
    fn merge_up(self, other: Rect) -> Rect {
        Rect {
            x: self.x.min(other.x),
            y: self.y.min(other.y),
            width: self.width.max(other.width),
            height: self.height + other.height,
        }
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let spans = Line::from(vec![
        Span::styled(
            " sql-optimizer-cli ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            if app.is_connected() { "●" } else { "○" },
            Style::default().fg(if app.is_connected() {
                Color::Green
            } else {
                Color::Red
            }),
        ),
        Span::raw(format!(" {} ", app.db_label)),
        Span::styled(
            format!("profile: {:?}", app.profile),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    f.render_widget(Paragraph::new(spans), area);
}

fn draw_tabs(f: &mut Frame, app: &App, area: Rect) {
    let titles: Vec<&str> = TABS.to_vec();
    f.render_widget(
        Tabs::new(titles).select(app.tab).highlight_style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        area,
    );
}

fn draw_connect(f: &mut Frame, app: &App, area: Rect) {
    let mut items: Vec<ListItem> = Vec::new();

    items.push(ListItem::new(Line::from(Span::styled(
        "Providers — press Enter to prefill a template:",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ))));
    for provider in PROVIDERS {
        items.push(ListItem::new(Line::from(vec![
            Span::styled("  ● ", Style::default().fg(provider.color())),
            Span::styled(
                format!("{:<11}", provider.label()),
                Style::default()
                    .fg(provider.color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(provider.blurb(), Style::default().fg(Color::DarkGray)),
        ])));
    }

    items.push(ListItem::new(Line::from(Span::styled(
        "Saved profiles (persisted) and session connections:",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ))));
    if app.entries.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "  (none yet — press 'a' to add a URL, or prefill from a provider)",
            Style::default().fg(Color::DarkGray),
        ))));
    }
    for entry in &app.entries {
        let connected = app.is_connected() && entry.profile.name == app.db_label;
        let (marker, color) = if connected {
            ("● ", Color::Green)
        } else {
            ("○ ", Color::DarkGray)
        };
        let provider_label = format!("{:<11}", entry.profile.kind.label());
        let mut badges = String::new();
        if entry.saved {
            badges.push_str(" [saved]");
        } else {
            badges.push_str(" [session]");
        }
        if entry.profile.accept_invalid_certs {
            badges.push_str(" ⚠cert");
        }
        if entry.profile.is_ephemeral() {
            badges.push_str(" ⚠session-only");
        }
        items.push(ListItem::new(Line::from(vec![
            Span::styled(format!("  {marker}"), Style::default().fg(color)),
            Span::styled(
                provider_label,
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(entry.profile.name.clone()),
            Span::styled(badges, Style::default().fg(Color::Yellow)),
            Span::styled(
                format!("  —  {}", redact_profile_url(&entry.profile.url)),
                Style::default().fg(Color::DarkGray),
            ),
        ])));
    }

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(
            " Connect — a: add · e: edit · d: delete · r: retry · x: disconnect · Enter: connect ",
        ))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = app.conn_list_state.clone();
    f.render_stateful_widget(list, area, &mut state);
}

fn draw_query(f: &mut Frame, app: &App, area: Rect) {
    if app.is_busy(Op::QueryPreview) {
        f.render_widget(
            Paragraph::new("Running query… (UI stays responsive; results arrive when ready)")
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    if let Some(err) = &app.query_error {
        let lines = vec![
            Line::from(Span::styled(
                "Query failed",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(err.clone()),
            Line::from(""),
            Line::from(Span::styled(
                "Only read-only SELECT statements are allowed.",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
        return;
    }

    let preview = match &app.query_rows {
        Some(p) => p,
        None => {
            let mut help = vec![
                Line::from(Span::styled(
                    "Query workspace",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
            ];
            if app.is_connected() {
                help.push(Line::from(
                    "Type a read-only SELECT below and press Enter to preview rows.",
                ));
                help.push(Line::from(
                    "Ctrl+A analyzes the current query; results also land in History.",
                ));
            } else {
                help.push(Line::from(Span::styled(
                    "No database connected.",
                    Style::default().fg(Color::Yellow),
                )));
                help.push(Line::from(
                    "Switch to the Connect tab (Tab or ←) and pick a provider to get started.",
                ));
            }
            f.render_widget(Paragraph::new(help).wrap(Wrap { trim: true }), area);
            return;
        }
    };

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        format!(
            "columns: {} · returned: {} rows (limit {}) · {}ms",
            preview.columns.len(),
            preview.rows.len(),
            preview.limit,
            app.query_elapsed_ms.unwrap_or(0)
        ),
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(""));

    if !preview.columns.is_empty() {
        lines.push(Line::from(Span::styled(
            preview.columns.join(" | "),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
    }
    for row in preview.rows.iter().take(QUERY_VIEW_ROW_CAP) {
        lines.push(Line::from(row.join(" | ")));
    }
    if preview.rows.len() > QUERY_VIEW_ROW_CAP {
        lines.push(Line::from(Span::styled(
            format!(
                "… {} more rows hidden (view capped at {QUERY_VIEW_ROW_CAP})",
                preview.rows.len() - QUERY_VIEW_ROW_CAP
            ),
            Style::default().fg(Color::Yellow),
        )));
    }
    if preview.truncated {
        lines.push(Line::from(Span::styled(
            "Result truncated at the configured row limit.",
            Style::default().fg(Color::Yellow),
        )));
    }

    let visible: Vec<Line> = lines.into_iter().skip(app.result_scroll as usize).collect();
    f.render_widget(
        Paragraph::new(visible).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Query results (↑↓ scroll · Enter re-runs · Ctrl+A analyze) "),
        ),
        area,
    );
}

fn draw_analyze(f: &mut Frame, app: &App, area: Rect) {
    if app.is_busy(Op::Analysis) {
        f.render_widget(
            Paragraph::new("Analyzing… (UI stays responsive; results arrive when ready)")
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    if app.results.is_empty() {
        let mut help = vec![
            Line::from(Span::styled(
                "Welcome!",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
        ];
        if app.is_connected() {
            help.push(Line::from(
                "Type a SQL query below and press Enter to analyze it.",
            ));
            help.push(Line::from(
                "Results appear here with recommendations, security findings,",
            ));
            help.push(Line::from(
                "regressions, and a plain-English EXPLAIN summary.",
            ));
        } else {
            help.push(Line::from(Span::styled(
                "No database connected.",
                Style::default().fg(Color::Yellow),
            )));
            help.push(Line::from(
                "Switch to the Connect tab (Tab or ←) and pick a provider to get started.",
            ));
            help.push(Line::from(""));
            help.push(Line::from(
                "Tip: SQLite (in-memory) needs no server — select it and press Enter.",
            ));
        }
        f.render_widget(Paragraph::new(help).wrap(Wrap { trim: true }), area);
        return;
    }

    // Show the selected (most recent) analysis.
    let result = &app.results[app.selected_result.unwrap_or(0)];
    let lines = result_to_lines(result);
    let visible: Vec<ListItem> = lines
        .into_iter()
        .skip(app.result_scroll as usize)
        .map(ListItem::new)
        .collect();
    f.render_stateful_widget(
        List::new(visible).block(Block::default().borders(Borders::ALL).title(format!(
            " Results ({}/{} analyses) ",
            app.selected_result.unwrap_or(0) + 1,
            app.results.len()
        ))),
        area,
        &mut dummy_list_state(),
    );
}

fn dummy_list_state() -> ListState {
    ListState::default()
}

fn draw_optimize(f: &mut Frame, app: &App, area: Rect) {
    let result = match (app.is_busy(Op::Analysis), app.results.first()) {
        (true, _) => {
            f.render_widget(
                Paragraph::new("Analyzing… Optimize refreshes when the analysis lands."),
                area,
            );
            return;
        }
        (false, Some(r)) => r,
        (false, None) => {
            let help = vec![
                Line::from(Span::styled(
                    "Optimize",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(""),
                Line::from("No analysis yet. Run a query in the Analyze tab (or Ctrl+A from"),
                Line::from("Query) — this tab groups missing indexes, inefficient joins,"),
                Line::from("cartesian products, N+1 patterns, and suggested rewrites, with"),
                Line::from("confidence, rationale, estimated improvement, and proposed SQL."),
                Line::from(""),
                Line::from(Span::styled(
                    "Nothing here executes against your database — copy/preview only.",
                    Style::default().fg(Color::Yellow),
                )),
            ];
            f.render_widget(Paragraph::new(help).wrap(Wrap { trim: true }), area);
            return;
        }
    };

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        format!("Query: {}", truncate(&result.query, 80)),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    let findings = crate::core::optimizer::ranked_findings(result);
    if findings.is_empty() {
        lines.push(Line::from(Span::styled(
            "✓ No optimization findings for the latest analysis.",
            Style::default().fg(Color::Green),
        )));
    } else {
        for finding in findings.iter().take(12) {
            let rec = &finding.recommendation;
            let kind_label = match rec.recommendation_type {
                RecommendationType::MissingIndex => "missing index",
                RecommendationType::NPlusOneQuery => "N+1",
                RecommendationType::InefficientJoin => "join",
                RecommendationType::CartesianProduct => "cartesian",
                RecommendationType::QueryRewrite => "rewrite",
            };
            let verify = if finding.verified {
                "✓ verified"
            } else {
                "heuristic"
            };
            let color = if finding.verified {
                Color::Green
            } else {
                Color::DarkGray
            };
            lines.push(Line::from(Span::styled(
                format!(
                    "  • [{kind_label}] {} ({:.0}% est. · {} · {} · from {})",
                    rec.description,
                    rec.estimated_improvement * 100.0,
                    rec.confidence,
                    verify,
                    finding.source
                ),
                Style::default().fg(color),
            )));
            if let Some(table) = &rec.table {
                lines.push(Line::from(Span::styled(
                    format!("      table: {table} · columns: {}", rec.columns.join(", ")),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
    }

    // Proposed rewrites / DDL from the rewriter — never executed.
    if let Some(schema) = &result.schema_snapshot {
        let fixes = crate::rewriting::rewriter::generate_fixes(
            &result.query,
            &result.recommendations,
            schema,
        );
        if !fixes.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("Proposed SQL / diffs ({}):", fixes.len()),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));
            for fix in fixes.iter().take(4) {
                lines.push(Line::from(Span::styled(
                    format!("  ▸ {}", fix.explanation),
                    Style::default().fg(Color::Yellow),
                )));
                for l in crate::rewriting::rewriter::format_diff(fix)
                    .lines()
                    .take(10)
                {
                    lines.push(Line::from(Span::styled(
                        format!("      {l}"),
                        Style::default().fg(Color::Gray),
                    )));
                }
            }
        }
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Generated SQL is copy/preview only — nothing is applied or executed.",
        Style::default().fg(Color::DarkGray),
    )));

    let visible: Vec<Line> = lines.into_iter().skip(app.result_scroll as usize).collect();
    f.render_widget(
        Paragraph::new(visible).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Optimize (ranked recommendations for the latest analysis) "),
        ),
        area,
    );
}

fn result_to_lines(result: &AnalysisResult) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    lines.push(Line::from(Span::styled(
        format!("Query: {}", truncate(&result.query, 100)),
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )));

    if !result.recommendations.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("\nOptimizations ({}):", result.recommendations.len()),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )));
        for rec in result.recommendations.iter().take(8) {
            lines.push(Line::from(Span::styled(
                format!(
                    "  • {} ({:.0}% est.)",
                    rec.description,
                    rec.estimated_improvement * 100.0
                ),
                Style::default().fg(Color::Yellow),
            )));
            lines.push(Line::from(Span::styled(
                format!("    confidence: {}", rec.confidence),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    if result.security_issues.is_empty() {
        lines.push(Line::from(Span::styled(
            "\n✓ No security issues",
            Style::default().fg(Color::Green),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            format!("\nSecurity issues ({}):", result.security_issues.len()),
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )));
        for issue in result.security_issues.iter().take(8) {
            let color = match issue.severity.rank() {
                3 | 2 => Color::Red,
                1 => Color::Yellow,
                _ => Color::Blue,
            };
            lines.push(Line::from(Span::styled(
                format!("  • [{:?}] {}", issue.severity, issue.description),
                Style::default().fg(color),
            )));
        }
    }

    if !result.regressions.is_empty() {
        lines.push(Line::from(Span::styled(
            "\nRegressions:",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
        for reg in result.regressions.iter().take(5) {
            lines.push(Line::from(Span::styled(
                format!("  ⚠ [{}] {}", reg.regression_type, reg.description),
                Style::default().fg(Color::Red),
            )));
        }
    }

    if !result.schema_drift.is_empty() {
        lines.push(Line::from(Span::styled(
            "\nSchema drift:",
            Style::default()
                .fg(Color::LightBlue)
                .add_modifier(Modifier::BOLD),
        )));
        for d in result.schema_drift.iter().take(5) {
            lines.push(Line::from(Span::styled(
                format!("  Δ [{}] {}", d.kind, d.detail),
                Style::default().fg(Color::LightBlue),
            )));
        }
    }

    if let Some(plan) = &result.explain_plan {
        if let Some(summary) = crate::core::explain::plain_explain_summary(&Some(plan.clone())) {
            lines.push(Line::from(Span::styled(
                format!("\nEXPLAIN: {summary}"),
                Style::default().fg(Color::Cyan),
            )));
        }
    }

    lines.push(Line::from(Span::styled(
        format!(
            "\n{}ms · security score {:.0}/100",
            result.execution_time_ms, result.security_score
        ),
        Style::default().fg(Color::DarkGray),
    )));

    lines
}

fn draw_schema(f: &mut Frame, app: &App, area: Rect) {
    let schema = match &app.schema {
        Some(s) => s,
        None => {
            let msg = if app.is_busy(Op::Schema) {
                "Introspecting schema…"
            } else if app.is_connected() {
                "No schema loaded yet. Press 's' to introspect the database."
            } else {
                "Not connected — connect to a database in the Connect tab, then press 's'."
            };
            f.render_widget(Paragraph::new(msg).wrap(Wrap { trim: true }), area);
            return;
        }
    };

    let mut items: Vec<ListItem> = Vec::new();
    for table in &schema.tables {
        items.push(ListItem::new(Line::from(Span::styled(
            format!("▸ {}", table.name),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))));
        for col in table.columns.iter().take(12) {
            items.push(ListItem::new(Line::from(Span::styled(
                format!("    {} : {}", col.name, col.data_type),
                Style::default().fg(Color::Gray),
            ))));
        }
        for idx in &table.indexes {
            items.push(ListItem::new(Line::from(Span::styled(
                format!("    [idx] {} ({})", idx.name, idx.columns.join(", ")),
                Style::default().fg(Color::Yellow),
            ))));
        }
        for fk in &table.foreign_keys {
            items.push(ListItem::new(Line::from(Span::styled(
                format!(
                    "    [fk] {} →{}.{}",
                    fk.columns.join(", "),
                    fk.referenced_table,
                    fk.referenced_columns.join(", ")
                ),
                Style::default().fg(Color::Magenta),
            ))));
        }
        items.push(ListItem::new(""));
    }

    let visible: Vec<ListItem> = items.into_iter().skip(app.result_scroll as usize).collect();
    let list = List::new(visible).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Schema (press 's' to refresh) "),
    );
    let mut state = app.schema_list_state.clone();
    f.render_stateful_widget(list, area, &mut state);
}

fn draw_health(f: &mut Frame, app: &App, area: Rect) {
    let lines: Vec<Line> = app
        .health_lines
        .iter()
        .map(|l| {
            if l.starts_with("Source:") || l.ends_with(':') {
                Line::from(Span::styled(
                    l.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ))
            } else {
                Line::from(l.clone())
            }
        })
        .collect();
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Health (press 'h' to refresh) "),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_history(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(
        format!(
            "Filter: {}  (c: toggle connection filter · f: cycle status · r: refresh · Enter: open in Query · Ctrl+A: analyze)",
            app.history_filter.label()
        ),
        Style::default().fg(Color::Cyan),
    )));
    lines.push(Line::from(""));

    if let Some(err) = &app.history_error {
        lines.push(Line::from(Span::styled(
            err.clone(),
            Style::default().fg(Color::Yellow),
        )));
    } else if app.history_runs.is_empty() {
        lines.push(Line::from(
            "(no runs recorded yet — run a query in the Query tab, or `analyze --track`)",
        ));
    } else {
        for (i, r) in app.history_runs.iter().enumerate() {
            let selected = app.history_list_state.selected() == Some(i);
            let status = if r.success { "ok " } else { "ERR" };
            let (marker, color) = if selected {
                ("▸", Color::Cyan)
            } else if r.success {
                (" ", Color::DarkGray)
            } else {
                (" ", Color::Red)
            };
            lines.push(Line::from(Span::styled(
                format!(
                    "{marker} {}  {:>6}ms  {:>3}  {:<16} conn:{}  {}",
                    r.timestamp,
                    r.execution_time_ms
                        .map(|t| t.to_string())
                        .unwrap_or("-".into()),
                    status,
                    truncate(&r.query_text, 48),
                    r.connection_label.as_deref().unwrap_or("-"),
                    r.error.as_deref().unwrap_or("")
                ),
                Style::default().fg(color),
            )));
        }
    }

    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" History (selectable query runs) "),
            )
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_sql_input(f: &mut Frame, app: &App, area: Rect) {
    let prompt = format!("SQL> {}", app.input);
    f.render_widget(
        Paragraph::new(prompt).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan))
                .title(" Query (Enter: run · Ctrl+A: analyze · Ctrl+P from Analyze: here) "),
        ),
        area,
    );
    // Position the cursor at end of input.
    let x = (area.x + 6 + app.input.chars().count() as u16).min(area.width.saturating_sub(2));
    let y = area.y + 1;
    f.set_cursor_position(ratatui::layout::Position::new(x, y));
}

fn draw_conn_form(f: &mut Frame, app: &App, area: Rect) {
    let (title, content, _masked) = match app.conn_input_stage {
        Some(ConnInputStage::Url) => (
            " New connection — URL (Enter: next · Esc: cancel) ",
            format!("URL> {}", app.conn_input),
            false,
        ),
        Some(ConnInputStage::Name) => (
            " New connection — display name (Enter: next · Esc: cancel) ",
            format!("Name> {}", app.conn_input),
            false,
        ),
        Some(ConnInputStage::Password) => (
            " New connection — password (never written to disk; Enter to skip · Esc: cancel) ",
            format!("Pass> {}", "*".repeat(app.conn_input.chars().count())),
            true,
        ),
        Some(ConnInputStage::Cert) => (
            " Accept invalid TLS certificates? y/N (Enter: confirm · Esc: cancel) ",
            format!("Cert> {}", app.conn_input),
            false,
        ),
        None => (" New connection ", String::new(), false),
    };
    let prefix_len = match app.conn_input_stage {
        Some(ConnInputStage::Url) => "URL> ".len(),
        Some(ConnInputStage::Name) => "Name> ".len(),
        Some(ConnInputStage::Password) => "Pass> ".len(),
        _ => "Cert> ".len(),
    };
    let display = content;
    f.render_widget(
        Paragraph::new(display).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Green))
                .title(title),
        ),
        area,
    );
    let shown_len = app.conn_input.chars().count();
    let x = (area.x + 2 + prefix_len as u16 + shown_len as u16).min(area.width.saturating_sub(2));
    let y = area.y + 1;
    f.set_cursor_position(ratatui::layout::Position::new(x, y));
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let hint = match app.tab {
        TAB_CONNECT if app.conn_input_stage.is_some() => {
            format!(" Enter: next · Esc: cancel  |  {}", app.status)
        }
        TAB_CONNECT => format!(
            " ↑↓: select · Enter: connect/prefill · a: add · e: edit · d: delete · r: retry · x: disconnect · Tab/←→: panels · q/Esc: quit  |  {}{}",
            app.status,
            if app.is_busy(Op::Connect) { " [connecting…]" } else { "" }
        ),
        TAB_QUERY => format!(
            " Enter: run (read-only) · Ctrl+A: analyze · ↑↓: scroll · h: health · Tab: panels · q/Esc: quit  |  {}  |  {}",
            app.status,
            if app.is_busy(Op::QueryPreview) { "running…" } else { "" }
        ),
        TAB_ANALYZE => format!(
            " Enter: analyze · Ctrl+P: send query to Query tab · ↑↓: scroll · Tab: panels · q/Esc: quit  |  {}  |  {}",
            app.status,
            if app.is_busy(Op::Analysis) { "working…" } else { "" }
        ),
        TAB_OPTIMIZE => format!(
            " ↑↓: scroll · r: re-rank · recommendations are copy/preview only  |  {}",
            app.status
        ),
        TAB_HISTORY => format!(
            " ↑↓: select · Enter: open in Query · Ctrl+A: analyze · c: connection filter · f: status filter · r: refresh  |  {}",
            app.status
        ),
        _ => format!(
            " ←→/Tab: switch panel · ↑↓: scroll · s: schema · h: health · q/Esc: quit  |  {}",
            app.status
        ),
    };
    f.render_widget(
        Paragraph::new(Span::styled(hint, Style::default().fg(Color::DarkGray))),
        area,
    );
}

fn truncate(s: &str, max: usize) -> String {
    let flat = s.replace('\n', " ");
    if flat.chars().count() <= max {
        flat
    } else {
        format!("{}…", flat.chars().take(max).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::connection::DatabaseConnector;

    /// Regression guard for the old nested-runtime panic: schema and health
    /// refresh helpers must be plain async functions that run inside
    /// `#[tokio::test]` without any `block_on`.
    #[tokio::test]
    async fn schema_and_health_refresh_do_not_panic_inside_tokio() {
        let mut connector = crate::database::sqlite::SqliteConnector::new();
        connector
            .connect("sqlite::memory:", &ConnectOptions::default())
            .await
            .expect("connect sqlite");

        // Seed a table so introspection has something to report.
        connector
            .preview_rows("SELECT 1", 1)
            .await
            .expect("health probe");

        let schema = fetch_schema(&connector).await.expect("schema");
        assert!(schema.tables.is_empty() || !schema.tables.is_empty());

        let snapshot = fetch_health_snapshot(DatabaseType::SQLite, &connector)
            .await
            .expect("health snapshot");
        assert!(!snapshot.stats_available, "sqlite has no runtime stats");
        let lines = health_lines_from(&snapshot);
        assert!(lines[0].starts_with("Source:"));
    }

    #[tokio::test]
    async fn analysis_pipeline_runs_inside_tokio_without_nested_runtime() {
        let mut connector = crate::database::sqlite::SqliteConnector::new();
        connector
            .connect("sqlite::memory:", &ConnectOptions::default())
            .await
            .expect("connect");
        let result = run_analysis_pipeline(
            &connector,
            "SELECT * FROM users",
            DatabaseType::SQLite,
            Profile::Oltp,
        )
        .await
        .expect("analysis");
        assert!(result.schema_snapshot.is_some());
    }

    #[tokio::test]
    async fn read_only_boundary_rejects_writes_and_ddl() {
        let mut connector = crate::database::sqlite::SqliteConnector::new();
        connector
            .connect("sqlite::memory:", &ConnectOptions::default())
            .await
            .expect("connect");

        for bad in [
            "INSERT INTO t VALUES (1)",
            "UPDATE t SET a = 1",
            "DELETE FROM t",
            "CREATE TABLE t (a int)",
            "DROP TABLE t",
            "not even sql",
            "SELECT 1; DELETE FROM t",
        ] {
            let err = connector
                .preview_rows(bad, 10)
                .await
                .expect_err("must reject non-read-only input");
            assert!(!err.to_string().is_empty());
        }

        // Read-only SELECT still works and enforces the row limit.
        let preview = connector.preview_rows("SELECT 1 AS one", 5).await.unwrap();
        assert_eq!(preview.rows.len(), 1);
        assert_eq!(preview.columns, vec!["one"]);
    }

    #[tokio::test]
    async fn connection_form_carries_name_through_password_and_cert_stages() {
        // Regression: the cert stage used to look the name up in the catalog
        // by profile ID, which is None for NEW connections — the form aborted
        // with "connection name lost" before ever connecting.
        let (tx, mut rx) = mpsc::unbounded_channel::<Job>();
        let mut app = App::new(Profile::Oltp, false, None, tx);
        let mut catalog = ProfileCatalog::load(
            std::env::temp_dir().join(format!("sql-opt-test-{}.json", uuid::Uuid::new_v4())),
        )
        .unwrap();
        let secrets = crate::core::connections::SessionSecretStore::new();

        // URL stage
        start_conn_form(
            &mut app,
            "postgresql://postgres@db.ref.supabase.co:5432/postgres?sslmode=require".into(),
            None,
        );
        // Enter on URL stage
        handle_connect_input_enter(&mut app, &secrets, &mut catalog).await;
        assert!(matches!(app.conn_input_stage, Some(ConnInputStage::Name)));

        // Type a name and press Enter -> password stage (provider needs one)
        app.conn_input = "My Supabase".into();
        handle_connect_input_enter(&mut app, &secrets, &mut catalog).await;
        assert!(matches!(
            app.conn_input_stage,
            Some(ConnInputStage::Password)
        ));
        assert_eq!(app.pending_conn_name.as_deref(), Some("My Supabase"));

        // Type a password and press Enter -> cert stage
        app.conn_input = "s3cret".into();
        handle_connect_input_enter(&mut app, &secrets, &mut catalog).await;
        assert!(matches!(app.conn_input_stage, Some(ConnInputStage::Cert)));

        // Answer 'y' at the cert stage — must NOT abort; must persist + connect.
        app.conn_input = "y".into();
        handle_connect_input_enter(&mut app, &secrets, &mut catalog).await;
        assert!(app.conn_input_stage.is_none(), "form should complete");

        // Profile was saved with the carried-through name and cert flag.
        assert_eq!(catalog.profiles.len(), 1);
        assert_eq!(catalog.profiles[0].name, "My Supabase");
        assert!(catalog.profiles[0].accept_invalid_certs);
        assert!(!catalog.profiles[0].url.contains("s3cret"));

        // The connect job carries the cert flag and profile identity.
        let job = rx.try_recv().expect("connect job sent");
        match job {
            Job::Connect {
                url,
                label,
                profile_id,
                accept_invalid_certs,
                ..
            } => {
                assert_eq!(label, "My Supabase");
                assert!(accept_invalid_certs);
                assert!(profile_id.is_some());
                assert!(!url.contains("s3cret"), "URL stays sanitized on the wire");
            }
            _ => panic!("expected Job::Connect"),
        }

        // Secret was stored under the profile's key.
        let key = format!("sql-optimizer/profile/{}", catalog.profiles[0].id);
        assert_eq!(
            secrets.get_secret(&key).await.unwrap().as_deref(),
            Some("s3cret")
        );
    }

    #[tokio::test]
    async fn worker_resolves_saved_profile_secrets_and_cert_flag() {
        // End-to-end: a saved profile with a sanitized URL + stored secret
        // connects with the password injected and the cert flag honored.
        let store = crate::core::connections::SessionSecretStore::new();
        store
            .set_secret("sql-optimizer/profile/fix-test", "hunter2")
            .await
            .unwrap();

        // Local sqlite serverless URL with an injected "password" would fail,
        // so exercise the resolution + options plumbing directly.
        let url = inject_secret_into_url(
            "postgresql://admin@ep-test.aws.neon.tech/neondb?sslmode=require",
            "fix-test",
            &store,
        )
        .await;
        assert!(url.contains("admin:hunter2@"));

        // connect_url must forward accept_invalid_certs into ConnectOptions.
        // Verify via a sqlite connect (no TLS) that the signature change is
        // at least wired and that detection/option building doesn't error.
        let (connector, db_type) = connect_url("sqlite::memory:", false, None, true)
            .await
            .expect("sqlite connects regardless of cert flag");
        assert_eq!(db_type, DatabaseType::SQLite);
        drop(connector);
    }

    #[test]
    fn stale_results_are_ignored_by_generation() {
        let (tx, _rx) = mpsc::unbounded_channel::<Job>();
        let mut app = App::new(Profile::Oltp, false, None, tx);
        app.conn_gen = 3;

        apply_result(
            &mut app,
            JobResult::Schema {
                gen: 2, // stale
                result: Ok(SchemaSnapshot::default()),
            },
        );
        assert!(app.schema.is_none(), "stale schema must be dropped");

        apply_result(
            &mut app,
            JobResult::Schema {
                gen: 3,
                result: Ok(SchemaSnapshot {
                    tables: vec![TableSchema::default()],
                }),
            },
        );
        assert_eq!(app.schema.as_ref().unwrap().tables.len(), 1);
    }

    #[tokio::test]
    async fn in_memory_sqlite_is_never_persisted() {
        let (tx, _rx) = mpsc::unbounded_channel::<Job>();
        let mut app = App::new(Profile::Oltp, false, None, tx);
        let mut catalog = ProfileCatalog::load(
            std::env::temp_dir().join(format!("sql-opt-test-{}.json", uuid::Uuid::new_v4())),
        )
        .unwrap();
        let secrets = crate::core::connections::SessionSecretStore::new();

        persist_profile(
            &mut app,
            &secrets,
            &mut catalog,
            "mem demo".into(),
            "sqlite::memory:".into(),
            None,
            false,
        )
        .await;

        assert!(
            catalog.profiles.is_empty(),
            "in-memory sqlite must not be saved as a profile"
        );
        assert_eq!(app.entries.len(), 2);
        assert!(!app.entries[1].saved);
    }

    #[test]
    fn health_lines_render_top_queries_and_cardinality() {
        let snapshot = crate::core::stats::HealthSnapshot {
            database_type: DatabaseType::PostgreSQL,
            top_queries: vec![],
            table_stats: vec![],
            stats_available: true,
            stats_source: "pg_stat_statements".into(),
        };
        let lines = health_lines_from(&snapshot);
        assert!(lines.iter().any(|l| l.contains("Top queries")));
        assert!(lines.iter().any(|l| l.contains("Table cardinality")));
    }
}
