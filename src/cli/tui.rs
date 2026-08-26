//! Phase "TUI": a terminal-user-interface dashboard for day-to-day use.
//!
//! Layout:
//! ┌──────────────────────────────────────────────────────────────┐
//! │ sql-optimizer-cli ● postgresql://…   profile: oltp           │  header
//! ├──────────────────────────────────────────────────────────────┤
//! │ [Analyze] [Schema] [Health] [History]                        │  tab bar
//! │                                                              │
//! │                 active tab content                           │
//! │                                                              │
//! ├──────────────────────────────────────────────────────────────┤
//! │ SQL> select * from users where email = 'x'                   │  input (Analyze)
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

const TABS: &[&str] = &["Analyze", "Schema", "Health", "History"];

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
}

impl App {
    fn new(profile: Profile) -> Self {
        Self {
            tab: 0,
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
            status: "Ready.".to_string(),
            running_analysis: false,
        }
    }
}

pub async fn run_tui(
    connection: &ConnectionArgs,
    simple_mode: bool,
    connect_timeout: Option<u64>,
    verbose: bool,
) -> Result<i32> {
    // Connect up-front so the whole session reuses one connector.
    let connect_result = if connection.has_connection() {
        let handler = crate::cli::commands::CommandHandler::new();
        match handler
            .connect_internal(connection, simple_mode, connect_timeout)
            .await
        {
            Ok(connector_and_type) => Some(connector_and_type),
            Err(e) => return Err(e.context("TUI requires a working database connection")),
        }
    } else {
        None
    };

    if connect_result.is_none() {
        anyhow::bail!(
            "The TUI needs a database connection. Pass --db or set SQL_OPTIMIZER_DB_URL."
        );
    }

    let (connector, db_type) = connect_result.unwrap();
    let _ = verbose;

    enable_raw_mode().context("Failed to enable raw mode (is this a terminal?)")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("Failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("Failed to create terminal")?;

    let mut app = Box::new(App::new(Profile::Oltp));
    app.db_type = Some(db_type);
    app.db_label = redact_url(&connection.resolve_connection_string().unwrap_or_default());

    let res = run_event_loop(&mut terminal, &mut app, connector).await;
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

async fn run_event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    connector: Box<dyn crate::database::connection::DatabaseConnector>,
) -> Result<i32> {
    loop {
        terminal.draw(|f| draw(f, app))?;

        // Non-blocking-ish wait for input.
        if !event::poll(std::time::Duration::from_millis(150))? {
            continue;
        }

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            match key.code {
                KeyCode::Esc => return Ok(0),
                KeyCode::Char('q') if key.modifiers.is_empty() && app.tab != 0 => return Ok(0),
                KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(0)
                }
                KeyCode::Tab | KeyCode::Right if !matches!(key.modifiers, KeyModifiers::SHIFT) => {
                    app.tab = (app.tab + 1) % TABS.len();
                    app.input.clear(); // right arrow doubles as text nav only in input
                }
                KeyCode::BackTab => app.tab = (app.tab + TABS.len() - 1) % TABS.len(),
                KeyCode::Left => {
                    app.tab = (app.tab + TABS.len() - 1) % TABS.len();
                }
                KeyCode::Down => {
                    app.result_scroll = app.result_scroll.saturating_add(1);
                    next_schema_item(app);
                }
                KeyCode::Up => {
                    app.result_scroll = app.result_scroll.saturating_sub(1);
                    prev_schema_item(app);
                }
                KeyCode::Char(c) if app.tab == 0 => {
                    // Analyze tab: typing goes into the query input.
                    match c {
                        'h' if app.input.is_empty() && key.modifiers.is_empty() => {
                            refresh_health(app, connector.as_ref());
                        }
                        _ => app.input.push(c),
                    }
                }
                KeyCode::Backspace if app.tab == 0 => {
                    app.input.pop();
                }
                KeyCode::Enter if app.tab == 0 && !app.running_analysis => {
                    let query = app.input.trim().to_string();
                    if query.is_empty() {
                        app.status = "Type a SQL query first.".into();
                        continue;
                    }
                    if query.eq_ignore_ascii_case("quit") || query.eq_ignore_ascii_case("exit") {
                        return Ok(0);
                    }
                    app.running_analysis = true;
                    app.status = "Analyzing…".into();

                    let db_type = app.db_type.unwrap_or(DatabaseType::SQLite);
                    let profile = app.profile.clone();
                    let analysis = std::panic::AssertUnwindSafe(async {
                        run_analysis(connector.as_ref(), query.clone(), db_type, profile).await
                    });
                    match analysis.0.await {
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
                    app.running_analysis = false;
                }
                KeyCode::Char('e') if app.tab == 0 && !app.input.is_empty() => {
                    app.status = "Tip: add EXPLAIN via CLI flag --explain; TUI shows the plan summary automatically when present.".into();
                }
                KeyCode::Char('s') if app.tab == 1 => {
                    refresh_schema(app, connector.as_ref());
                }
                KeyCode::Char('h') if app.tab == 2 => {
                    refresh_health(app, connector.as_ref());
                }
                KeyCode::Char('r') if app.tab == 3 => {
                    refresh_history(app);
                }
                _ => {}
            }
        }
    }
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
            Constraint::Length(3), // input (analyze tab)
            Constraint::Length(1), // footer
        ])
        .split(f.area());

    draw_header(f, app, chunks[0]);
    draw_tabs(f, app, chunks[1]);

    match app.tab {
        0 => draw_analyze(f, app, chunks[2]),
        1 => draw_schema(f, app, chunks[2]),
        2 => draw_health(f, app, chunks[2]),
        _ => draw_history(f, app, chunks[2]),
    }

    if app.tab == 0 {
        draw_input(f, app, chunks[3]);
    }

    draw_footer(
        f,
        app,
        if app.tab == 0 {
            chunks[4]
        } else {
            chunks[3].merge_up(chunks[4])
        },
    );
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
            if app.db_type.is_some() { "●" } else { "○" },
            Style::default().fg(if app.db_type.is_some() {
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

fn draw_analyze(f: &mut Frame, app: &App, area: Rect) {
    if app.results.is_empty() {
        let help = Paragraph::new(vec![
            Line::from(Span::styled(
                "Welcome!",
                Style::default().add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from("Type a SQL query below and press Enter to analyze it."),
            Line::from("Results appear here with recommendations, security findings,"),
            Line::from("regressions, and a plain-English EXPLAIN summary."),
        ])
        .wrap(Wrap { trim: true });
        f.render_widget(help, area);
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
            f.render_widget(
                Paragraph::new("No schema loaded yet. Press 's' to introspect the database.")
                    .wrap(Wrap { trim: true }),
                area,
            );
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

fn draw_input(f: &mut Frame, app: &App, area: Rect) {
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
    let inner = area;
    let x = (inner.x + 6 + app.input.chars().count() as u16).min(inner.width.saturating_sub(2));
    let y = inner.y + 1;
    f.set_cursor_position(ratatui::layout::Position::new(x, y));
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let hint = if app.tab == 0 {
        format!(
            " Tab: switch panel · Enter: analyze · ↑↓: scroll · h: health · q/Esc: quit   |   {}   |   {}",
            app.status,
            if app.running_analysis { "working…" } else { "" }
        )
    } else {
        format!(
            " ←→/Tab: switch panel · ↑↓: scroll · s: schema · h: health · r: history · q/Esc: quit   |   {}",
            app.status
        )
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
