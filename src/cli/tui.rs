//! Phase "TUI": a terminal-user-interface dashboard for day-to-day use.
//!
//! The TUI always launches — even with no working database connection. The
//! Connect tab lets you visually pick or add a database (SQLite, PostgreSQL,
//! MySQL, Supabase, Neon, or any custom URL); connections persist in a
//! catalog for the rest of the session and can be switched at any time.
//!
//! Layout:
//! ┌──────────────────────────────────────────────────────────────┐
//! │ sql-optimizer-cli ● postgresql://…   profile: oltp           │  header
//! ├──────────────────────────────────────────────────────────────┤
//! │ [Connect] [Analyze] [Schema] [Health] [History]              │  tab bar
//! │                                                              │
//! │                 active tab content                           │
//! │                                                              │
//! ├──────────────────────────────────────────────────────────────┤
//! │ SQL> select * from users where email = 'x'                   │  input (Analyze / Connect forms)
//! ├──────────────────────────────────────────────────────────────┤
//! │ Tab: switch · Enter: run · ↑↓: scroll · e: explain · q/Esc   │  footer
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
use std::io;

use crate::cli::ConnectionArgs;
use crate::core::types::*;

const TABS: &[&str] = &["Connect", "Analyze", "Schema", "Health", "History"];

const TAB_CONNECT: usize = 0;
const TAB_ANALYZE: usize = 1;
const TAB_SCHEMA: usize = 2;
const TAB_HEALTH: usize = 3;
const TAB_HISTORY: usize = 4;

// Connect-tab list geometry: rows 1..=5 = the five provider presets,
// row 6 = "Session connections" header, rows 7.. = user-added catalog entries.
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

    /// Template connection string used to prefill the add-connection form.
    fn template(self) -> &'static str {
        match self {
            Provider::Sqlite => "sqlite::memory:",
            Provider::Postgres => "postgresql://user:password@localhost:5432/postgres?sslmode=require",
            Provider::Mysql => "mysql://user:password@localhost:3306/mydb",
            Provider::Supabase => {
                "postgresql://postgres:password@db.<project-ref>.supabase.co:5432/postgres?sslmode=require"
            }
            Provider::Neon => {
                "postgresql://user:password@ep-<endpoint>.<region>.aws.neon.tech/neondb?sslmode=require"
            }
        }
    }

    fn blurb(self) -> &'static str {
        match self {
            Provider::Sqlite => "zero-setup — in-memory or a local .db file",
            Provider::Postgres => "local or self-hosted server",
            Provider::Mysql => "local or self-hosted server",
            Provider::Supabase => "Postgres-compatible — use the session pooler URL",
            Provider::Neon => "Postgres-compatible — serverless connection string",
        }
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

/// A connection saved in the catalog. Lives for the whole TUI session.
#[derive(Debug, Clone)]
struct ConnectionEntry {
    name: String,
    url: String,
    provider: Option<Provider>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnInputStage {
    /// Collecting/editing the connection URL.
    Url,
    /// URL accepted; collecting a display name.
    Name,
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
    history_lines: Vec<String>,
    db_type: Option<DatabaseType>,
    db_label: String,
    profile: Profile,
    status: String,
    running_analysis: bool,

    // Connection state: `connector` is replaceable so sessions can switch
    // databases from the Connect tab without restarting the TUI.
    connector: Option<Box<dyn crate::database::connection::DatabaseConnector>>,

    // Connect-tab catalog + form state.
    connections: Vec<ConnectionEntry>,
    conn_list_state: ListState,
    conn_input: String,
    conn_input_stage: Option<ConnInputStage>,
    pending_conn_url: String,

    // Connection options captured from the CLI invocation.
    simple_mode: bool,
    connect_timeout: Option<u64>,
}

impl App {
    fn new(profile: Profile, simple_mode: bool, connect_timeout: Option<u64>) -> Self {
        // Start the session catalog with one always-available entry.
        let connections = vec![ConnectionEntry {
            name: "SQLite (in-memory demo)".to_string(),
            url: "sqlite::memory:".to_string(),
            provider: Some(Provider::Sqlite),
        }];
        Self {
            tab: TAB_CONNECT,
            input: String::new(),
            results: Vec::new(),
            selected_result: None,
            result_scroll: 0,
            schema: None,
            schema_list_state: ListState::default(),
            health_lines: vec!["Press 'h' on this tab to refresh the health snapshot.".into()],
            history_lines: vec!["(no tracked runs yet — use `analyze --track`)".into()],
            db_type: None,
            db_label: "(not connected)".to_string(),
            profile,
            status: "Not connected — pick a database in the Connect tab.".to_string(),
            running_analysis: false,
            connector: None,
            connections,
            conn_list_state: ListState::default(),
            conn_input: String::new(),
            conn_input_stage: None,
            pending_conn_url: String::new(),
            simple_mode,
            connect_timeout,
        }
    }

    fn is_connected(&self) -> bool {
        self.connector.is_some()
    }
}

pub async fn run_tui(
    connection: &ConnectionArgs,
    simple_mode: bool,
    connect_timeout: Option<u64>,
    verbose: bool,
) -> Result<i32> {
    let mut app = Box::new(App::new(Profile::Oltp, simple_mode, connect_timeout));
    let _ = verbose;

    // Try the CLI-provided connection, but never hard-fail: on error we land
    // on the Connect tab with the URL added to the catalog so it can be
    // retried or edited interactively.
    if connection.has_connection() {
        let cli_url = connection.resolve_connection_string().ok();
        let handler = crate::cli::commands::CommandHandler::new();
        match handler
            .connect_internal(connection, simple_mode, connect_timeout)
            .await
        {
            Ok((connector, db_type)) => {
                app.connector = Some(connector);
                app.db_type = Some(db_type);
                if let Some(url) = &cli_url {
                    app.db_label = redact_url(url);
                    app.connections.push(ConnectionEntry {
                        name: "CLI-provided connection".to_string(),
                        url: url.clone(),
                        provider: provider_from_url(url),
                    });
                }
                app.tab = TAB_ANALYZE;
                app.status = "Connected. Type a SQL query below and press Enter.".into();
            }
            Err(e) => {
                app.tab = TAB_CONNECT;
                app.status = format!(
                    "Connection failed: {e}. Pick or add a database below — Enter connects, 'a' adds a URL."
                );
                if let Some(url) = &cli_url {
                    app.connections.push(ConnectionEntry {
                        name: "CLI-provided connection (failed)".to_string(),
                        url: url.clone(),
                        provider: provider_from_url(url),
                    });
                    app.conn_list_state
                        .select(Some(SESSION_FIRST_ROW + app.connections.len() - 1));
                }
            }
        }
    } else {
        app.tab = TAB_CONNECT;
        app.conn_list_state.select(Some(PROVIDER_FIRST_ROW));
        app.status =
            "Not connected — choose a provider below, or press 'a' to add a connection URL."
                .into();
    }

    enable_raw_mode().context("Failed to enable raw mode (is this a terminal?)")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("Failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("Failed to create terminal")?;

    let res = run_event_loop(&mut terminal, &mut app).await;
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

fn redact_url(url: &str) -> String {
    // Strip credentials: scheme://user:pass@host/db -> scheme://user@host/db
    if let Some(scheme_end) = url.find("://") {
        let rest = &url[scheme_end + 3..];
        if let Some(at) = rest.find('@') {
            let creds = &rest[..at];
            let user = creds.split(':').next().unwrap_or("");
            return format!("{}://{}@{}", &url[..scheme_end], user, &rest[at + 1..]);
        }
    }
    url.to_string()
}

/// Connect to a catalog URL, replacing the current connector on success.
/// Failure keeps the existing connection (if any) and reports hints.
async fn connect_entry(app: &mut App, url: String) {
    let url = url.trim().to_string();
    if url.is_empty() {
        app.status = "Connection URL is empty.".into();
        return;
    }
    if provider_from_url(&url).is_none() {
        app.status = "Unrecognized URL. Must start with postgresql://, mysql://, or sqlite:// \
                      (or end with .db / .sqlite)."
            .into();
        return;
    }

    app.status = format!("Connecting to {}…", redact_url(&url));
    let handler = crate::cli::commands::CommandHandler::new();
    let conn_args = ConnectionArgs {
        db: Some(url.clone()),
        ..Default::default()
    };
    match handler
        .connect_internal(&conn_args, app.simple_mode, app.connect_timeout)
        .await
    {
        Ok((connector, db_type)) => {
            app.connector = Some(connector);
            app.db_type = Some(db_type);
            app.db_label = redact_url(&url);
            // Reset per-connection state so stale schema/health from a
            // previous database is never shown.
            app.schema = None;
            app.schema_list_state.select(None);
            app.health_lines =
                vec!["Press 'h' on this tab to refresh the health snapshot.".into()];
            app.tab = TAB_ANALYZE;
            app.status = format!("Connected to {} ({}).", redact_url(&url), db_type_name(db_type));
        }
        Err(e) => {
            let mut msg = format!("Connection failed: {e}");
            let lower = e.to_string().to_lowercase();
            if lower.contains("certificate") || lower.contains("tls") {
                msg.push_str(
                    " — TLS/certificate issue: check your system clock, the sslmode parameter, \
                     or restart with --accept-invalid-certs for self-signed certs.",
                );
            } else if lower.contains("timeout") || lower.contains("refused") {
                msg.push_str(" — check host, port, and network/firewall access.");
            }
            app.status = msg;
        }
    }
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
    let redacted = redact_url(rest);
    format!("{} · {}", provider.label(), redacted)
}

fn is_valid_conn_row(row: usize, entries: usize) -> bool {
    (PROVIDER_FIRST_ROW..=PROVIDER_LAST_ROW).contains(&row)
        || (SESSION_FIRST_ROW..SESSION_FIRST_ROW + entries).contains(&row)
}

fn move_conn_selection(app: &mut App, forward: bool) {
    let entries = app.connections.len();
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

/// Enter on the Connect tab: prefill the add-connection form from a provider
/// preset, or connect to the selected catalog entry.
fn handle_connect_enter(app: &mut App) {
    let row = app.conn_list_state.selected().unwrap_or(PROVIDER_FIRST_ROW);

    if (PROVIDER_FIRST_ROW..=PROVIDER_LAST_ROW).contains(&row) {
        let provider = PROVIDERS[row - PROVIDER_FIRST_ROW];
        app.conn_input = provider.template().to_string();
        app.pending_conn_url.clear();
        app.conn_input_stage = Some(ConnInputStage::Url);
        app.status = format!(
            "{}: edit the URL and press Enter (Esc cancels).",
            provider.label()
        );
    } else if let Some(entry) = app
        .connections
        .get(row - SESSION_FIRST_ROW)
        .cloned()
    {
        // Trigger the async connect; run via the event loop's block_on-free
        // path by storing it as the pending action.
        app.pending_conn_url = entry.url;
    }
}

async fn handle_connect_input_enter(app: &mut App) {
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
                    app.conn_input = default_entry_name(&url, provider);
                    app.conn_input_stage = Some(ConnInputStage::Name);
                    app.status = "Name this connection and press Enter to connect.".into();
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
            let url = std::mem::take(&mut app.pending_conn_url);
            let provider = provider_from_url(&url);
            app.connections.push(ConnectionEntry {
                name,
                url,
                provider,
            });
            app.conn_list_state
                .select(Some(SESSION_FIRST_ROW + app.connections.len() - 1));
            app.conn_input_stage = None;
            app.conn_input.clear();
            let url = app.connections.last().unwrap().url.clone();
            connect_entry(app, url).await;
        }
    }
}

fn delete_selected_connection(app: &mut App) {
    let row = app.conn_list_state.selected().unwrap_or(0);
    let idx = match row.checked_sub(SESSION_FIRST_ROW) {
        Some(i) if i < app.connections.len() => i,
        _ => return,
    };
    let name = app.connections[idx].name.clone();
    app.connections.remove(idx);
    let entries = app.connections.len();
    let new_row = if entries == 0 {
        PROVIDER_LAST_ROW
    } else {
        SESSION_FIRST_ROW + idx.min(entries - 1)
    };
    app.conn_list_state.select(Some(new_row));
    app.status = format!("Removed '{name}' from the session catalog.");
}

async fn run_event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<i32> {
    loop {
        // Consume a pending connect action triggered from list mode.
        let pending = if app.pending_conn_url.is_empty() || app.conn_input_stage.is_some() {
            None
        } else {
            Some(std::mem::take(&mut app.pending_conn_url))
        };
        if let Some(url) = pending {
            connect_entry(app, url).await;
            if app.connector.is_some() {
                continue; // redraw immediately in the new state
            }
        }

        terminal.draw(|f| draw(f, app))?;

        // Non-blocking-ish wait for input.
        if !event::poll(std::time::Duration::from_millis(150))? {
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
                    app.status = "Connection form cancelled.".into();
                }
                KeyCode::Esc => return Ok(0),
                KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(0)
                }
                KeyCode::Char('q')
                    if key.modifiers.is_empty()
                        && app.tab != TAB_ANALYZE
                        && !in_conn_form =>
                {
                    return Ok(0)
                }
                KeyCode::Tab | KeyCode::Right
                    if !matches!(key.modifiers, KeyModifiers::SHIFT) && !in_conn_form =>
                {
                    app.tab = (app.tab + 1) % TABS.len();
                    app.input.clear(); // right arrow doubles as text nav only in input
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
                    } else {
                        app.result_scroll = app.result_scroll.saturating_add(1);
                        next_schema_item(app);
                    }
                }
                KeyCode::Up => {
                    if app.tab == TAB_CONNECT && !in_conn_form {
                        move_conn_selection(app, false);
                    } else {
                        app.result_scroll = app.result_scroll.saturating_sub(1);
                        prev_schema_item(app);
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
                    handle_connect_input_enter(app).await;
                }
                KeyCode::Enter if app.tab == TAB_CONNECT => {
                    handle_connect_enter(app);
                }
                KeyCode::Char('a') if app.tab == TAB_CONNECT && !in_conn_form => {
                    app.conn_input.clear();
                    app.conn_input_stage = Some(ConnInputStage::Url);
                    app.status =
                        "Enter a connection URL (postgresql://, mysql://, sqlite://) and press Enter."
                            .into();
                }
                KeyCode::Char('d') if app.tab == TAB_CONNECT && !in_conn_form => {
                    delete_selected_connection(app);
                }

                // ---- Analyze tab ----
                KeyCode::Char(c) if app.tab == TAB_ANALYZE => {
                    // Analyze tab: typing goes into the query input.
                    match c {
                        'h' if app.input.is_empty() && key.modifiers.is_empty() => {
                            with_connector(app, |app, connector| refresh_health(app, connector));
                        }
                        _ => app.input.push(c),
                    }
                }
                KeyCode::Backspace if app.tab == TAB_ANALYZE => {
                    app.input.pop();
                }
                KeyCode::Enter if app.tab == TAB_ANALYZE && !app.running_analysis => {
                    let query = app.input.trim().to_string();
                    if query.is_empty() {
                        app.status = "Type a SQL query first.".into();
                        continue;
                    }
                    if query.eq_ignore_ascii_case("quit") || query.eq_ignore_ascii_case("exit") {
                        return Ok(0);
                    }
                    if app.connector.is_none() {
                        app.status = "Not connected — switch to the Connect tab (Tab/←) and pick \
                                      a database first."
                            .into();
                        continue;
                    }
                    app.running_analysis = true;
                    app.status = "Analyzing…".into();

                    let db_type = app.db_type.unwrap_or(DatabaseType::SQLite);
                    let profile = app.profile.clone();
                    let connector = app.connector.take();
                    let result = match connector.as_deref() {
                        Some(conn) => {
                            run_analysis(conn, query.clone(), db_type, profile).await
                        }
                        None => Err(anyhow::anyhow!("No database connection.")),
                    };
                    match result {
                        Ok(result) => {
                            app.results.insert(0, result);
                            app.selected_result = Some(0);
                            app.result_scroll = 0;
                            app.status = format!("Analyzed: {}", truncate(&query, 50));
                        }
                        Err(e) => {
                            app.status = format!("Error: {}", e);
                        }
                    }
                    app.connector = connector;
                    app.running_analysis = false;
                }
                KeyCode::Char('e')
                    if app.tab == TAB_ANALYZE && !app.input.is_empty() =>
                {
                    app.status = "Tip: add EXPLAIN via CLI flag --explain; TUI shows the plan summary automatically when present.".into();
                }

                // ---- Other tabs ----
                KeyCode::Char('s') if app.tab == TAB_SCHEMA => {
                    with_connector(app, |app, connector| refresh_schema(app, connector));
                }
                KeyCode::Char('h') if app.tab == TAB_HEALTH => {
                    with_connector(app, |app, connector| refresh_health(app, connector));
                }
                KeyCode::Char('r') if app.tab == TAB_HISTORY => {
                    refresh_history(app);
                }
                _ => {}
            }
        }
    }
}

/// Run a connector-using helper while the connector is temporarily moved out
/// of `app` to avoid overlapping borrows.
fn with_connector(
    app: &mut App,
    f: impl FnOnce(&mut App, &dyn crate::database::connection::DatabaseConnector),
) {
    let connector = app.connector.take();
    match connector.as_deref() {
        Some(conn) => f(app, conn),
        None => {
            app.status = "Not connected — switch to the Connect tab (Tab/←) and pick a database \
                          first."
                .into();
        }
    }
    app.connector = connector;
}

async fn run_analysis(
    connector: &dyn crate::database::connection::DatabaseConnector,
    query: String,
    db_type: DatabaseType,
    profile: Profile,
) -> Result<AnalysisResult> {
    let analyzer = crate::core::analyzer::SqlAnalyzer::new();
    let mut result = analyzer.analyze_query(&query, db_type, profile).await?;

    let schema = connector.introspect_schema().await?;
    result.schema_snapshot = Some(schema);
    analyzer.run_schema_checks(&mut result).await?;

    // Best-effort plan capture for the plain-English summary.
    if let Ok(plan) = connector.explain_query(&query).await {
        result.explain_plan = Some(plan);
    }

    Ok(result)
}

fn refresh_schema(app: &mut App, connector: &dyn crate::database::connection::DatabaseConnector) {
    let rt = tokio::runtime::Handle::current();
    match rt.block_on(connector.introspect_schema()) {
        Ok(schema) => {
            let tables = schema.tables.len();
            app.schema = Some(schema);
            app.status = format!("Schema refreshed: {} tables", tables);
        }
        Err(e) => app.status = format!("Schema introspection failed: {}", e),
    }
}

fn refresh_health(app: &mut App, connector: &dyn crate::database::connection::DatabaseConnector) {
    let db_type = match app.db_type {
        Some(t) => t,
        None => return,
    };

    // Reuse the command handler's snapshot logic through a tiny local copy.
    let lines: Vec<String> = match fetch_health(db_type, connector) {
        Ok(snapshot) => {
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
        Err(e) => vec![format!("Health check failed: {}", e)],
    };
    app.health_lines = lines;
    app.status = "Health snapshot refreshed.".into();
}

fn fetch_health(
    db_type: DatabaseType,
    connector: &dyn crate::database::connection::DatabaseConnector,
) -> Result<crate::core::stats::HealthSnapshot> {
    use crate::core::stats as sm;
    let rt = tokio::runtime::Handle::current();

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

    let available = rt
        .block_on(connector.preview_rows(&check_sql, 1))
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

    let top = rt
        .block_on(connector.preview_rows(&top_sql, 20))
        .map(|p| sm::parse_query_stat_rows(&p))
        .unwrap_or_default();
    let tables = rt
        .block_on(connector.preview_rows(&table_sql, 100))
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

fn refresh_history(app: &mut App) {
    if !crate::core::regression::StateStore::default_exists() {
        app.history_lines = vec![
            "No state store (.sql-optimizer/history.sqlite). Run `analyze --track` first.".into(),
        ];
        return;
    }
    match crate::core::regression::StateStore::open(".sql-optimizer/history.sqlite") {
        Ok(store) => match store.get_recent_runs(30) {
            Ok(runs) => {
                app.history_lines = runs
                    .iter()
                    .map(|r| {
                        format!(
                            "{}  {:>6}ms  idx:{:<20} {}",
                            r.timestamp,
                            r.execution_time_ms
                                .map(|t| t.to_string())
                                .unwrap_or("-".into()),
                            r.index_used.as_deref().unwrap_or("-"),
                            truncate(&r.query_text, 60)
                        )
                    })
                    .collect();
                if app.history_lines.is_empty() {
                    app.history_lines = vec!["(no runs recorded yet)".into()];
                }
            }
            Err(e) => app.history_lines = vec![format!("Failed to read history: {}", e)],
        },
        Err(e) => app.history_lines = vec![format!("Failed to open state store: {}", e)],
    }
    app.status = "History refreshed.".into();
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

fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Length(3), // tabs
            Constraint::Min(5),    // content
            Constraint::Length(3), // input (analyze tab / connect form)
            Constraint::Length(1), // footer
        ])
        .split(f.area());

    draw_header(f, app, chunks[0]);
    draw_tabs(f, app, chunks[1]);

    match app.tab {
        TAB_CONNECT => draw_connect(f, app, chunks[2]),
        TAB_ANALYZE => draw_analyze(f, app, chunks[2]),
        TAB_SCHEMA => draw_schema(f, app, chunks[2]),
        TAB_HEALTH => draw_health(f, app, chunks[2]),
        _ => draw_history(f, app, chunks[2]),
    }

    let show_sql_input = app.tab == TAB_ANALYZE;
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
        "Session connections (persist for this TUI session):",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ))));
    if app.connections.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "  (none yet — press 'a' to add a URL, or prefill from a provider)",
            Style::default().fg(Color::DarkGray),
        ))));
    }
    for entry in &app.connections {
        let connected = app.is_connected() && redact_url(&entry.url) == app.db_label;
        let (marker, color) = if connected {
            ("● ", Color::Green)
        } else {
            ("○ ", Color::DarkGray)
        };
        let provider_label = entry
            .provider
            .map(|p| format!("{:<11}", p.label()))
            .unwrap_or_else(|| format!("{:<11}", "Custom"));
        items.push(ListItem::new(Line::from(vec![
            Span::styled(format!("  {marker}"), Style::default().fg(color)),
            Span::styled(
                provider_label,
                Style::default()
                    .fg(entry.provider.map(|p| p.color()).unwrap_or(Color::Gray))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(entry.name.clone()),
            Span::styled(
                format!("  —  {}", redact_url(&entry.url)),
                Style::default().fg(Color::DarkGray),
            ),
        ])));
    }

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Connect — choose a database (a: add · d: delete) "),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = app.conn_list_state.clone();
    f.render_stateful_widget(list, area, &mut state);
}

fn draw_analyze(f: &mut Frame, app: &App, area: Rect) {
    if app.results.is_empty() {
        let mut help = vec![
            Line::from(Span::styled(
                "Welcome!",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
        ];
        if app.is_connected() {
            help.push(Line::from("Type a SQL query below and press Enter to analyze it."));
            help.push(Line::from(
                "Results appear here with recommendations, security findings,",
            ));
            help.push(Line::from("regressions, and a plain-English EXPLAIN summary."));
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
        f.render_widget(
            Paragraph::new(help).wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    // Show the selected (most recent) analysis.
    let result = &app.results[app.selected_result.unwrap_or(0)];
    let lines = result_to_lines(result);
    f.render_stateful_widget(
        List::new(lines.into_iter().map(ListItem::new).collect::<Vec<_>>())
            .block(Block::default().borders(Borders::ALL).title(format!(
                " Results ({}/{} shown) ",
                app.results.len(),
                app.results.len()
            )))
            .highlight_style(Style::default()),
        area,
        &mut dummy_list_state(),
    );
    // Scroll rendering: Paragraph would be simpler but List keeps colors; apply offset manually.
    let _ = app.result_scroll;
}

fn dummy_list_state() -> ListState {
    ListState::default()
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
                format!("\nEXPLAIN: {}", summary),
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
            let msg = if app.is_connected() {
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
                    "    [fk] {} → {}.{}",
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
    let lines: Vec<Line> = app
        .history_lines
        .iter()
        .map(|l| Line::from(l.clone()))
        .collect();
    f.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Run History (press 'r' to refresh) "),
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
                .title(" Query "),
        ),
        area,
    );
    // Position the cursor at end of input.
    let x = (area.x + 6 + app.input.chars().count() as u16).min(area.width.saturating_sub(2));
    let y = area.y + 1;
    f.set_cursor_position(ratatui::layout::Position::new(x, y));
}

fn draw_conn_form(f: &mut Frame, app: &App, area: Rect) {
    let (title, content) = match app.conn_input_stage {
        Some(ConnInputStage::Url) => (
            " New connection — URL (Enter: next · Esc: cancel) ",
            format!("URL> {}", app.conn_input),
        ),
        Some(ConnInputStage::Name) => (
            " New connection — display name (Enter: connect · Esc: cancel) ",
            format!("Name> {}", app.conn_input),
        ),
        None => (" New connection ", String::new()),
    };
    let prefix_len = match app.conn_input_stage {
        Some(ConnInputStage::Url) => "URL> ".len(),
        _ => "Name> ".len(),
    };
    f.render_widget(
        Paragraph::new(content).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Green))
                .title(title),
        ),
        area,
    );
    let x = (area.x + 2 + prefix_len as u16 + app.conn_input.chars().count() as u16)
        .min(area.width.saturating_sub(2));
    let y = area.y + 1;
    f.set_cursor_position(ratatui::layout::Position::new(x, y));
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let hint = match app.tab {
        TAB_CONNECT if app.conn_input_stage.is_some() => format!(
            " Enter: next · Esc: cancel  |  {}",
            app.status
        ),
        TAB_CONNECT => format!(
            " ↑↓: select · Enter: connect/prefill · a: add URL · d: delete · Tab/←→: panels · q/Esc: quit  |  {}",
            app.status
        ),
        TAB_ANALYZE => format!(
            " Tab: switch panel · Enter: analyze · ↑↓: scroll · h: health · q/Esc: quit   |   {}   |   {}",
            app.status,
            if app.running_analysis { "working…" } else { "" }
        ),
        _ => format!(
            " ←→/Tab: switch panel · ↑↓: scroll · s: schema · h: health · r: history · q/Esc: quit   |   {}",
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
