use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::{fs, io::Write};

use crate::cli::output::OutputFormatter;
use crate::cli::ConnectionArgs;
use crate::core::analyzer::SqlAnalyzer;
use crate::core::annotations::{render_annotations, AnnotationFormat};
use crate::core::baseline::Baseline;
use crate::core::fingerprint::fingerprint;
use crate::core::types::{
    AnalysisResult, ConnectOptions, DatabaseType, OutputFormat, Profile, SchemaSnapshot, Severity,
};
use crate::database::connection::{create_connector, DatabaseConnector};

/// Resolved Phase 7 CI options (CLI flags layered over `.sql-optimizer.toml`).
#[derive(Debug, Clone, Default)]
pub struct CiOptions {
    pub fail_on: Option<Severity>,
    pub annotate: Option<AnnotationFormat>,
    pub baseline: Option<PathBuf>,
    pub save_baseline: Option<PathBuf>,
}

/// Distinct exit codes (Phase 7).
pub const EXIT_CLEAN: i32 = 0;
pub const EXIT_WARNINGS_ONLY: i32 = 1;
pub const EXIT_BLOCKING: i32 = 2;

pub struct CommandHandler {
    analyzer: SqlAnalyzer,
}

impl CommandHandler {
    pub fn new() -> Self {
        Self {
            analyzer: SqlAnalyzer::new(),
        }
    }

    /// Detect database type from a connection URL.
    pub fn detect_db_type(db_url: &str) -> Result<DatabaseType> {
        if db_url.starts_with("postgresql") || db_url.starts_with("postgres") {
            Ok(DatabaseType::PostgreSQL)
        } else if db_url.starts_with("mysql") {
            Ok(DatabaseType::MySQL)
        } else if db_url.starts_with("sqlite")
            || db_url.ends_with(".db")
            || db_url.ends_with(".sqlite")
        {
            Ok(DatabaseType::SQLite)
        } else {
            Err(anyhow!(
                "Unsupported database URL format. Must start with postgresql://, mysql://, or sqlite://"
            ))
        }
    }

    /// Build and connect a connector for the given URL.
    async fn connect(
        &self,
        connection: &ConnectionArgs,
        simple_mode: bool,
        connect_timeout: Option<u64>,
    ) -> Result<(Box<dyn DatabaseConnector>, DatabaseType)> {
        let db_url = connection.resolve_connection_string()?;
        let db_type = Self::detect_db_type(&db_url)?;
        let mut connector = create_connector(db_type);
        let options = ConnectOptions {
            simple_mode,
            connect_timeout_secs: connect_timeout,
            accept_invalid_certs: connection.accept_invalid_certs,
        };
        connector.connect(&db_url, &options).await?;
        Ok((connector, db_type))
    }

    /// Public alias used by the TUI to share the same connection flow.
    pub async fn connect_internal(
        &self,
        connection: &ConnectionArgs,
        simple_mode: bool,
        connect_timeout: Option<u64>,
    ) -> Result<(Box<dyn DatabaseConnector>, DatabaseType)> {
        self.connect(connection, simple_mode, connect_timeout).await
    }

    /// Full analysis of one query against an optional live schema snapshot.
    async fn analyze_with_schema(
        &self,
        query: &str,
        db_type: DatabaseType,
        profile: Profile,
        schema: Option<&SchemaSnapshot>,
    ) -> Result<AnalysisResult> {
        let mut result = self.analyzer.analyze_query(query, db_type, profile).await?;
        if let Some(schema) = schema {
            result.schema_snapshot = Some(schema.clone());
            let analyzer = SqlAnalyzer::new();
            analyzer.run_schema_checks(&mut result).await?;
        }
        Ok(result)
    }

    /// Apply Phase 7 CI plumbing to one or more results:
    /// baseline filtering, annotation emission. Returns the (possibly
    /// filtered) results for further output/exit-code evaluation.
    fn apply_ci_plumbing(
        &self,
        results: Vec<AnalysisResult>,
        ci: &CiOptions,
        origins: &[Option<crate::core::annotations::Origin>],
    ) -> Result<Vec<AnalysisResult>> {
        let mut results = results;

        // Baseline diffing: report only new findings.
        if let Some(baseline_path) = &ci.baseline {
            let base = Baseline::load(baseline_path)?;
            results = base.filter_new_findings(&results);
            println!(
                "Baseline applied from {}: reporting only new findings.",
                baseline_path.display()
            );
        }

        // Annotation emission is additive — it never replaces normal output.
        if let Some(fmt) = ci.annotate {
            let pairs: Vec<(Option<crate::core::annotations::Origin>, &AnalysisResult)> = results
                .iter()
                .enumerate()
                .map(|(i, r)| (origins.get(i).cloned().flatten(), r))
                .collect();
            print!("{}", render_annotations(&pairs, fmt));
        }

        // Save new baseline.
        if let Some(save_path) = &ci.save_baseline {
            Baseline::save(save_path, &results)?;
            println!("Baseline saved to {}", save_path.display());
        }

        Ok(results)
    }

    /// Compute the process exit code from analyzed results.
    fn evaluate_exit_code(&self, results: &[AnalysisResult], ci: &CiOptions) -> i32 {
        let any_findings = results.iter().any(|r| r.has_findings());
        let max_sev = results
            .iter()
            .filter_map(effective_max_severity)
            .max_by(|a, b| a.rank().cmp(&b.rank()));

        match &ci.fail_on {
            None => {
                if any_findings {
                    EXIT_WARNINGS_ONLY
                } else {
                    EXIT_CLEAN
                }
            }
            Some(threshold) => {
                let blocking = max_sev
                    .map(|s| s.rank() >= threshold.rank())
                    .unwrap_or(false);
                if blocking {
                    EXIT_BLOCKING
                } else if any_findings {
                    EXIT_WARNINGS_ONLY
                } else {
                    EXIT_CLEAN
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn handle_analyze(
        &self,
        query: &str,
        connection: &ConnectionArgs,
        explain: bool,
        show_rows: bool,
        row_limit: usize,
        output_format: OutputFormat,
        verbose: bool,
        simple_mode: bool,
        connect_timeout: Option<u64>,
        profile: Profile,
        track: bool,
        schema_baseline: Option<PathBuf>,
        ci: CiOptions,
    ) -> Result<i32> {
        let (connector, db_type) = self
            .connect(connection, simple_mode, connect_timeout)
            .await?;

        if verbose {
            eprintln!("Query: {}", query);
        }

        let mut result = self
            .analyze_with_schema(query, db_type, profile.clone(), None)
            .await?;

        let schema = connector.introspect_schema().await?;
        result.schema_snapshot = Some(schema);

        if show_rows {
            match connector.preview_rows(query, row_limit).await {
                Ok(preview) => result.row_preview = Some(preview),
                Err(e) => eprintln!("Warning: row preview unavailable: {}", e),
            }
        }

        // Run schema-dependent checks (missing index, etc.)
        let analyzer = SqlAnalyzer::new();
        analyzer.run_schema_checks(&mut result).await?;

        // Show execution plan if requested
        if explain {
            match connector.explain_query(query).await {
                Ok(plan) => result.explain_plan = Some(plan),
                Err(e) => eprintln!("Warning: explain unavailable: {}", e),
            }
        }

        // Schema drift detection against a stored snapshot.
        if let Some(bp) = resolve_baseline_path(schema_baseline.as_deref()) {
            let live = if let Some(snap) = result.schema_snapshot.as_ref() {
                snap.clone()
            } else {
                connector.introspect_schema().await?
            };
            apply_schema_drift(&mut result, &bp, &live).await?;
        }

        // Phase 3.8: regression tracking
        let should_track = track || crate::core::regression::StateStore::default_exists();
        if should_track {
            let plan_summary = result
                .explain_plan
                .as_ref()
                .and_then(|p| crate::core::explain::plain_explain_summary(&Some(p.clone())));
            let index_used = result
                .explain_plan
                .as_ref()
                .and_then(|p| p.root.as_ref().and_then(|r| r.index_used.clone()));

            match crate::core::regression::StateStore::open_default() {
                Ok(store) => {
                    let regressions = store.detect_regressions(
                        query,
                        Some(result.execution_time_ms),
                        plan_summary.as_deref(),
                        index_used.as_deref(),
                    );

                    if let Ok(regs) = regressions {
                        for reg in regs {
                            result.regressions.push(crate::core::types::RegressionInfo {
                                regression_type: format!("{:?}", reg.regression_type),
                                description: reg.description,
                                current_value: reg.current_value,
                                previous_value: reg.previous_value,
                            });
                        }
                    }

                    let _ = store.record_run(
                        query,
                        Some(result.execution_time_ms),
                        None,
                        plan_summary.as_deref(),
                        index_used.as_deref(),
                    );
                }
                Err(e) => {
                    eprintln!(
                        "Warning: could not open state store for regression tracking: {}",
                        e
                    );
                }
            }
        }

        // Phase 7 CI plumbing
        let results = self.apply_ci_plumbing(vec![result], &ci, &[None])?;
        let result = &results[0];

        // Format and output results
        let is_text = matches!(output_format, OutputFormat::Text);
        let formatter = OutputFormatter::new(output_format.clone());
        if is_text {
            formatter.format(result)?;
        } else {
            let rendered = formatter.render(result)?;
            let path = write_auto_output("analyze", query, &output_format, &rendered)?;
            println!("Results written to {}", path.display());
        }

        Ok(self.evaluate_exit_code(results.as_slice(), &ci))
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn handle_interactive(
        &self,
        history_file: &PathBuf,
        connection: &ConnectionArgs,
        show_rows: bool,
        row_limit: usize,
        output_format: OutputFormat,
        simple_mode: bool,
        connect_timeout: Option<u64>,
        profile: Profile,
    ) -> Result<i32> {
        use dialoguer::Input;
        use std::fs::OpenOptions;
        use std::io::Write as _;

        let (connector, db_type) = self
            .connect(connection, simple_mode, connect_timeout)
            .await?;

        let db_url = connection.resolve_connection_string()?;

        println!("SQL Optimizer Interactive Mode");
        println!(
            "Connected to: {}",
            &db_url[..std::cmp::min(db_url.len(), 20)]
        );
        println!("Type 'exit' to quit, 'help' for commands\n");

        let mut history = Vec::new();

        // Try to load existing history
        if let Ok(file) = std::fs::read_to_string(history_file) {
            history = file.lines().map(|s| s.to_string()).collect();
        }

        loop {
            let query = Input::<String>::new()
                .with_prompt("sql-optimizer")
                .interact_text()?;

            if query.to_lowercase().trim() == "exit" {
                break;
            }

            if query.to_lowercase().trim() == "help" {
                println!("Commands:");
                println!("  exit - Exit interactive mode");
                println!("  help - Show this help");
                println!("  Any SQL query will be analyzed");
                continue;
            }

            if query.trim().is_empty() {
                continue;
            }

            // Add to history
            history.push(query.clone());

            // Analyze the query
            match self
                .analyzer
                .analyze_query(&query, db_type, profile.clone())
                .await
            {
                Ok(result) => {
                    let mut result = result;
                    if show_rows {
                        match connector.preview_rows(&query, row_limit).await {
                            Ok(preview) => result.row_preview = Some(preview),
                            Err(e) => eprintln!("Warning: row preview unavailable: {}", e),
                        }
                    }

                    let formatter = OutputFormatter::new(output_format.clone());
                    if matches!(output_format, OutputFormat::Text) {
                        formatter.format(&result)?;
                    } else {
                        let rendered = formatter.render(&result)?;
                        let path =
                            write_auto_output("interactive", &query, &output_format, &rendered)?;
                        println!("Results written to {}", path.display());
                    }
                    println!(); // Add spacing between results
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                }
            }
        }

        // Save history
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(history_file)
        {
            for line in history.iter().rev().take(100) {
                // Keep last 100 queries
                writeln!(file, "{}", line)?;
            }
        }

        Ok(EXIT_CLEAN)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn handle_batch(
        &self,
        input_file: &PathBuf,
        output_file: &Option<PathBuf>,
        output_format: OutputFormat,
        connection: &ConnectionArgs,
        simple_mode: bool,
        connect_timeout: Option<u64>,
        profile: Profile,
        schema_baseline: Option<PathBuf>,
        ci: CiOptions,
    ) -> Result<i32> {
        let (connector, db_type) = self
            .connect(connection, simple_mode, connect_timeout)
            .await?;

        println!("Processing batch file: {:?}", input_file);

        let content = fs::read_to_string(input_file)?;
        let queries: Vec<&str> = content
            .lines()
            .filter(|line| !line.trim().is_empty() && !line.trim().starts_with("--"))
            .collect();

        println!("Found {} queries to analyze", queries.len());

        let schema = connector.introspect_schema().await?;
        let analyzer = SqlAnalyzer::new();

        let mut results = Vec::new();

        for (i, query) in queries.iter().enumerate() {
            println!("Analyzing query {}/{}", i + 1, queries.len());

            match self
                .analyze_with_schema(query, db_type, profile.clone(), Some(&schema))
                .await
            {
                Ok(mut result) => {
                    analyzer.run_schema_checks(&mut result).await?;
                    results.push(result);
                }
                Err(e) => eprintln!("Error analyzing query {}: {}", i + 1, e),
            }
        }

        // Schema drift detection against a stored snapshot.
        if !results.is_empty() {
            if let Some(bp) = resolve_baseline_path(schema_baseline.as_deref()) {
                let live = if let Some(snap) = results[0].schema_snapshot.as_ref() {
                    snap.clone()
                } else {
                    connector.introspect_schema().await?
                };
                apply_schema_drift(&mut results[0], &bp, &live).await?;
            }
        }

        let origins = vec![None; results.len()];
        let results = self.apply_ci_plumbing(results, &ci, &origins)?;

        let json_output = serde_json::to_string_pretty(&results)?;
        if let Some(output_file) = output_file {
            fs::write(output_file, json_output)?;
            println!(
                "Batch analysis complete. Results written to: {:?}",
                output_file
            );
        } else if !matches!(output_format, OutputFormat::Text) {
            let rendered = match output_format {
                OutputFormat::Json => json_output,
                OutputFormat::Yaml => serde_yaml::to_string(&results)?,
                OutputFormat::Markdown => {
                    let mut rendered = String::new();
                    for (i, result) in results.iter().enumerate() {
                        if i > 0 {
                            rendered.push_str("\n\n");
                        }
                        rendered.push_str(
                            &OutputFormatter::new(OutputFormat::Markdown).render(result)?,
                        );
                    }
                    rendered
                }
                OutputFormat::Text => unreachable!(),
            };
            let path = write_auto_output("batch", &content, &output_format, &rendered)?;
            println!("Results written to {}", path.display());
            println!("Batch analysis complete.");
        } else {
            println!("Batch analysis complete.");
        }

        Ok(self.evaluate_exit_code(results.as_slice(), &ci))
    }

    pub async fn handle_schema(
        &self,
        connection: &ConnectionArgs,
        simple_mode: bool,
        connect_timeout: Option<u64>,
        save: Option<PathBuf>,
    ) -> Result<i32> {
        let (connector, _db_type) = self
            .connect(connection, simple_mode, connect_timeout)
            .await?;
        let schema = connector.introspect_schema().await?;

        println!("Schema snapshot (tables={}):", schema.tables.len());
        for table in schema.tables.iter() {
            println!("- {}", table.name);
            for col in &table.columns {
                println!("  - {} : {}", col.name, col.data_type);
            }
            for idx in &table.indexes {
                println!("  [idx] {} ({})", idx.name, idx.columns.join(", "));
            }
            for fk in &table.foreign_keys {
                println!(
                    "  [fk] {} ({}) -> {} ({})",
                    fk.name,
                    fk.columns.join(", "),
                    fk.referenced_table,
                    fk.referenced_columns.join(", ")
                );
            }
        }

        if let Some(path) = save {
            let json = serde_json::to_string_pretty(&schema)?;
            if let Some(parent) = path.parent() {
                if !parent.as_os_str().is_empty() && !parent.exists() {
                    fs::create_dir_all(parent)?;
                }
            }
            fs::write(&path, json)?;
            println!("Schema snapshot saved to {}", path.display());
        }

        Ok(EXIT_CLEAN)
    }

    pub async fn handle_health(
        &self,
        connection: &ConnectionArgs,
        simple_mode: bool,
        connect_timeout: Option<u64>,
    ) -> Result<i32> {
        use colored::*;

        let (connector, db_type) = self
            .connect(connection, simple_mode, connect_timeout)
            .await?;

        println!("{}", "Database Health Snapshot".bold().cyan());
        println!("{}", "========================".bold().cyan());
        println!("Database type: {:?}", db_type);
        println!();

        // Schema overview
        let schema = connector.introspect_schema().await?;
        println!("{}", "Schema Overview:".bold().yellow());
        println!("  Tables: {}", schema.tables.len());
        let total_cols: usize = schema.tables.iter().map(|t| t.columns.len()).sum();
        let total_indexes: usize = schema.tables.iter().map(|t| t.indexes.len()).sum();
        let total_fks: usize = schema.tables.iter().map(|t| t.foreign_keys.len()).sum();
        println!("  Total columns: {}", total_cols);
        println!("  Total indexes: {}", total_indexes);
        println!("  Total foreign keys: {}", total_fks);
        println!();

        // Per-table stats
        println!("{}", "Table Details:".bold().yellow());
        for table in &schema.tables {
            println!(
                "  {} ({} cols, {} idx, {} FKs)",
                table.name,
                table.columns.len(),
                table.indexes.len(),
                table.foreign_keys.len(),
            );
        }
        println!();

        // Live runtime stats with graceful degradation (design decision 14).
        println!("{}", "Runtime Stats:".bold().yellow());
        match fetch_health_snapshot(connector.as_ref(), db_type) {
            Ok(snapshot) => {
                if snapshot.stats_available {
                    println!("  Source: {} (live)", snapshot.stats_source.bold().green());
                    println!("  Top queries by total time:");
                    for stat in snapshot.top_queries.iter().take(10) {
                        println!(
                            "    [{:>8} calls | {:>10.1} ms total | {:>10} rows] {}",
                            stat.calls,
                            stat.total_time_ms,
                            stat.rows_returned,
                            truncate_line(&stat.query, 80)
                        );
                    }
                    if snapshot.top_queries.is_empty() {
                        println!("    (no statements recorded yet)");
                    }
                    println!();
                    println!("  Table cardinality:");
                    for ts in snapshot.table_stats.iter().take(15) {
                        print!(
                            "    {:<40} ~{} rows",
                            truncate_line(&ts.table_name, 40),
                            ts.estimated_rows
                        );
                        if let Some(bytes) = ts.index_size_bytes {
                            print!(" (~{} bytes)", bytes);
                        }
                        println!();
                    }
                } else {
                    println!("  {}", snapshot.stats_source.yellow());
                    println!("  Runtime stats are not available on this instance;");
                    println!("  recommendations fall back to static/AST-based confidence.");
                    print_enablement_hint(db_type);
                }
            }
            Err(e) => {
                println!("  {}", format!("Runtime stats unavailable: {}", e).yellow());
                println!("  Falling back to static analysis confidence.");
                print_enablement_hint(db_type);
            }
        }

        Ok(EXIT_CLEAN)
    }

    /// Phase 5: project-wide scanning. Never prompts (§1.3).
    #[allow(clippy::too_many_arguments)]
    pub async fn handle_scan(
        &self,
        path: &Path,
        connection: &ConnectionArgs,
        output_format: OutputFormat,
        output_file: Option<PathBuf>,
        simple_mode: bool,
        connect_timeout: Option<u64>,
        profile: Profile,
        exclude: Vec<String>,
        schema_baseline: Option<PathBuf>,
        ci: CiOptions,
    ) -> Result<i32> {
        use colored::*;

        let scanner = crate::scan::Scanner::new().with_exclude(exclude);
        let extracted = scanner.scan(path)?;

        if extracted.is_empty() {
            println!(
                "{}",
                format!("No SQL queries found under {}.", path.display()).yellow()
            );
            return Ok(EXIT_CLEAN);
        }

        println!(
            "{}",
            format!(
                "Scanning {} — extracted {} SQL statements",
                path.display(),
                extracted.len()
            )
            .bold()
            .cyan()
        );

        // Optional DB connection enables schema-aware analysis of every shape.
        let (schema, db_type) = if connection.has_connection() {
            let (connector, db_type) = self
                .connect(connection, simple_mode, connect_timeout)
                .await?;
            let schema = connector.introspect_schema().await?;
            (Some(schema), Some(db_type))
        } else {
            (None, None)
        };

        let pairs: Vec<(String, crate::scan::Origin)> =
            extracted.into_iter().map(|q| (q.text, q.origin)).collect();

        let schema_ref = schema.as_ref();
        let db_type_unwrapped = db_type;
        let report = tokio::task::block_in_place(|| {
            crate::scan::report::build_report(&path.to_string_lossy(), pairs, move |query| {
                let db_type = db_type_unwrapped?;
                let rt = tokio::runtime::Handle::current();
                let profile = profile.clone();
                rt.block_on(async move {
                    match self
                        .analyze_with_schema(query, db_type, profile, schema_ref)
                        .await
                    {
                        Ok(mut result) => {
                            if schema_ref.is_some() {
                                let _ = SqlAnalyzer::new().run_schema_checks(&mut result).await;
                            }
                            Some(result)
                        }
                        Err(_) => None,
                    }
                })
            })
        });

        // Schema drift detection against a stored snapshot.
        let mut drift_results: Vec<AnalysisResult> = report
            .entries
            .iter()
            .filter_map(|e| e.result.clone())
            .collect();
        if !drift_results.is_empty() && connection.has_connection() {
            // Only introspect when a baseline actually exists to diff against.
            let baseline_path = match schema_baseline.as_deref() {
                Some(p) => Some(p.to_path_buf()),
                None => {
                    let p = Path::new(".sql-optimizer/schema-snapshot.json");
                    p.exists().then(|| p.to_path_buf())
                }
            };
            if let Some(bp) = baseline_path {
                let (connector, _) = self
                    .connect(connection, simple_mode, connect_timeout)
                    .await?;
                let live = if let Some(snap) = drift_results[0].schema_snapshot.as_ref() {
                    snap.clone()
                } else {
                    connector.introspect_schema().await?
                };
                apply_schema_drift(&mut drift_results[0], &bp, &live).await?;
            }
        }

        // Text summary always prints (unless JSON/YAML goes to stdout).
        let full_report_json = serde_json::to_string_pretty(&report)?;

        match (&output_file, &output_format) {
            (None, OutputFormat::Json) => println!("{}", full_report_json),
            (None, OutputFormat::Yaml) => println!("{}", serde_yaml::to_string(&report)?),
            (None, OutputFormat::Markdown) => {
                println!("{}", report.render_summary());
                let path = write_auto_output(
                    "scan",
                    &full_report_json,
                    &OutputFormat::Markdown,
                    &render_scan_markdown(&report),
                )?;
                println!("Full report written to {}", path.display());
            }
            (None, OutputFormat::Text) => println!("{}", report.render_summary()),
            (Some(file), fmt) => {
                println!("{}", report.render_summary());
                let rendered = match fmt {
                    OutputFormat::Json => full_report_json,
                    OutputFormat::Yaml => serde_yaml::to_string(&report)?,
                    OutputFormat::Markdown => render_scan_markdown(&report),
                    OutputFormat::Text => report.render_summary(),
                };
                fs::write(file, rendered)?;
                println!("Full report written to {}", file.display());
            }
        }

        // CI plumbing: annotations over findings with real file origins.
        if let Some(ann_fmt) = ci.annotate {
            let pairs = report.results_with_origins();
            print!("{}", render_annotations(&pairs, ann_fmt));
        }
        if let Some(baseline_path) = &ci.baseline {
            let base = Baseline::load(baseline_path)?;
            let current: Vec<AnalysisResult> = report
                .entries
                .iter()
                .filter_map(|e| e.result.clone())
                .collect();
            let new_only = base.filter_new_findings(&current);
            let new_count: usize = new_only.iter().map(count_result_findings).sum();
            println!(
                "Baseline applied: {} new finding(s) out of {}.",
                new_count,
                current.iter().map(count_result_findings).sum::<usize>()
            );
        }
        if let Some(save_path) = &ci.save_baseline {
            let current: Vec<AnalysisResult> = report
                .entries
                .iter()
                .filter_map(|e| e.result.clone())
                .collect();
            Baseline::save(save_path, &current)?;
            println!("Baseline saved to {}", save_path.display());
        }

        // Exit code evaluation over all analyzed entries (+ drift).
        let mut all_results: Vec<AnalysisResult> = drift_results.clone();
        all_results.extend(report.entries.iter().filter_map(|e| e.result.clone()));
        let any_findings = all_results.iter().any(|r| r.has_findings());
        let max_sev = all_results
            .iter()
            .filter_map(effective_max_severity)
            .max_by(|a, b| a.rank().cmp(&b.rank()));

        Ok(match &ci.fail_on {
            None => {
                if any_findings {
                    EXIT_WARNINGS_ONLY
                } else {
                    EXIT_CLEAN
                }
            }
            Some(threshold) => {
                if max_sev
                    .map(|s| s.rank() >= threshold.rank())
                    .unwrap_or(false)
                {
                    EXIT_BLOCKING
                } else if any_findings {
                    EXIT_WARNINGS_ONLY
                } else {
                    EXIT_CLEAN
                }
            }
        })
    }
}

fn count_result_findings(r: &AnalysisResult) -> usize {
    r.security_issues.len() + r.recommendations.len() + r.regressions.len() + r.schema_drift.len()
}

/// Worst severity including regressions (High) and material drift (Medium).
fn effective_max_severity(result: &AnalysisResult) -> Option<Severity> {
    let mut worst = result.max_severity();
    if !result.regressions.is_empty() {
        worst = Some(
            worst
                .take()
                .map(|w| {
                    if w.rank() < Severity::High.rank() {
                        Severity::High
                    } else {
                        w
                    }
                })
                .unwrap_or(Severity::High),
        );
    }
    if !result.schema_drift.is_empty() {
        let drift_rank = Severity::Medium.rank();
        worst = Some(match worst.take() {
            Some(w) if w.rank() >= drift_rank => w,
            _ => Severity::Medium,
        });
    }
    worst
}

/// Resolve which schema-baseline file to diff against, if any: an explicit
/// `--schema-baseline <path>` wins; otherwise fall back to the opt-in default
/// location per design decision 11's spirit.
fn resolve_baseline_path(explicit: Option<&Path>) -> Option<PathBuf> {
    match explicit {
        Some(p) => Some(p.to_path_buf()),
        None => {
            let p = Path::new(".sql-optimizer/schema-snapshot.json");
            p.exists().then(|| p.to_path_buf())
        }
    }
}

/// Diff the live schema against a stored baseline snapshot and record drift in
/// the result. The caller is responsible for providing a fresh introspection
/// (or an already-captured snapshot) so we only hit the database when a
/// baseline actually exists.
async fn apply_schema_drift(
    result: &mut AnalysisResult,
    baseline_path: &Path,
    fresh: &SchemaSnapshot,
) -> Result<()> {
    let old_content = fs::read_to_string(baseline_path)
        .with_context(|| format!("Failed to read schema baseline {}", baseline_path.display()))?;
    let old: SchemaSnapshot = serde_json::from_str(&old_content).with_context(|| {
        format!(
            "Failed to parse schema baseline {} as a SchemaSnapshot JSON",
            baseline_path.display()
        )
    })?;

    let drift = crate::core::schema_diff::diff_schemas(&old, fresh);
    if !drift.is_empty() {
        eprintln!(
            "Schema drift detected ({} change(s) since baseline {}):",
            drift.len(),
            baseline_path.display()
        );
        for d in &drift {
            eprintln!("  [{}] {}", d.kind, d.detail);
        }
    }
    result.schema_drift = drift;
    Ok(())
}

/// Fetch a live health snapshot via the generic read-only query channel.
/// Every failure degrades to `stats_available=false` rather than failing.
fn fetch_health_snapshot(
    connector: &dyn DatabaseConnector,
    db_type: DatabaseType,
) -> Result<crate::core::stats::HealthSnapshot> {
    use crate::core::stats as stats_mod;

    let rt = tokio::runtime::Handle::current();

    match db_type {
        DatabaseType::PostgreSQL => {
            let exists_preview = rt.block_on(async {
                connector.preview_rows(stats_mod::PG_STAT_EXISTS_SQL.trim(), 1).await
            });
            let available = exists_preview
                .ok()
                .and_then(|p| p.rows.first().and_then(|r| r.first().cloned()))
                .map(|v| stats_mod::parse_pg_stats_available(Some(&v)))
                .unwrap_or(false);

            if !available {
                return Ok(crate::core::stats::HealthSnapshot {
                    database_type: db_type,
                    top_queries: vec![],
                    table_stats: vec![],
                    stats_available: false,
                    stats_source: "pg_stat_statements extension not installed or not visible"
                        .to_string(),
                });
            }

            let top = rt.block_on(async {
                connector
                    .preview_rows(stats_mod::PG_STAT_STATEMENTS_SQL.trim(), 20)
                    .await
            });
            let tables = rt.block_on(async {
                connector
                    .preview_rows(stats_mod::PG_TABLE_STATS_SQL.trim(), 100)
                    .await
            });

            Ok(crate::core::stats::HealthSnapshot {
                database_type: db_type,
                top_queries: top
                    .map(|p| stats_mod::parse_query_stat_rows(&p))
                    .unwrap_or_default(),
                table_stats: tables
                    .map(|p| stats_mod::parse_table_stat_rows(&p))
                    .unwrap_or_default(),
                stats_available: true,
                stats_source: "pg_stat_statements".to_string(),
            })
        }
        DatabaseType::MySQL => {
            let enabled_preview = rt.block_on(async {
                connector
                    .preview_rows(stats_mod::MYSQL_PERF_SCHEMA_CHECK_SQL.trim(), 1)
                    .await
            });
            let available = enabled_preview
                .ok()
                .and_then(|p| p.rows.first().and_then(|r| r.first().cloned()))
                .map(|v| stats_mod::parse_mysql_stats_available(Some(&v)))
                .unwrap_or(false);

            if !available {
                return Ok(crate::core::stats::HealthSnapshot {
                    database_type: db_type,
                    top_queries: vec![],
                    table_stats: vec![],
                    stats_available: false,
                    stats_source: "performance_schema is not enabled".to_string(),
                });
            }

            let top = rt.block_on(async {
                connector
                    .preview_rows(stats_mod::MYSQL_PERF_SCHEMA_SQL.trim(), 20)
                    .await
            });
            let tables = rt.block_on(async {
                connector
                    .preview_rows(stats_mod::MYSQL_TABLE_STATS_SQL.trim(), 100)
                    .await
            });

            Ok(crate::core::stats::HealthSnapshot {
                database_type: db_type,
                top_queries: top
                    .map(|p| stats_mod::parse_query_stat_rows(&p))
                    .unwrap_or_default(),
                table_stats: tables
                    .map(|p| stats_mod::parse_table_stat_rows(&p))
                    .unwrap_or_default(),
                stats_available: true,
                stats_source: "performance_schema.events_statements_summary_by_digest".to_string(),
            })
        }
        DatabaseType::SQLite => Ok(crate::core::stats::HealthSnapshot {
            database_type: db_type,
            top_queries: vec![],
            table_stats: vec![],
            stats_available: false,
            stats_source:
                "SQLite has no runtime query-stats extension; EXPLAIN QUERY PLAN covers single queries"
                    .to_string(),
        }),
    }
}

fn print_enablement_hint(db_type: DatabaseType) {
    match db_type {
        DatabaseType::PostgreSQL => {
            println!("  To enable: CREATE EXTENSION IF NOT EXISTS pg_stat_statements;");
            println!("  (requires shared_preload_libraries to include pg_stat_statements)");
        }
        DatabaseType::MySQL => {
            println!(
                "  To enable: SET GLOBAL performance_schema = ON; (server restart may be required)"
            );
        }
        DatabaseType::SQLite => {}
    }
}

fn truncate_line(s: &str, max: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= max {
        s
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

fn render_scan_markdown(report: &crate::scan::report::ScanReport) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(&mut out, "# Project Scan Report: {}", report.root);
    let _ = writeln!(&mut out);
    let _ = writeln!(
        &mut out,
        "- Queries extracted: **{}**",
        report.total_queries_extracted
    );
    let _ = writeln!(&mut out, "- Unique shapes: **{}**", report.unique_shapes);
    let _ = writeln!(&mut out);
    let _ = writeln!(&mut out, "## Top Offenders");
    for g in &report.top_offenders {
        let _ = writeln!(
            &mut out,
            "- `{}` — {} occurrence(s), {} finding(s), worst severity: {}",
            &g.fingerprint[..12],
            g.occurrences,
            g.findings,
            g.worst_severity.as_deref().unwrap_or("none")
        );
        let _ = writeln!(
            &mut out,
            "  - Example: `{}`",
            truncate_line(&g.example_query, 120)
        );
    }
    out
}

fn write_auto_output(
    job_type: &str,
    query: &str,
    format: &OutputFormat,
    contents: &str,
) -> Result<std::path::PathBuf> {
    let extension = match format {
        OutputFormat::Json => "json",
        OutputFormat::Yaml => "yaml",
        OutputFormat::Markdown => "md",
        OutputFormat::Text => return Err(anyhow!("text output is not auto-written")),
    };

    let prefix = &fingerprint(query)[..8];
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S").to_string();
    let file_name = format!("{}_{}_{}.{}", job_type, prefix, timestamp, extension);
    let output_dir = Path::new("output");
    fs::create_dir_all(output_dir)?;
    let path = output_dir.join(file_name);
    let mut file = fs::File::create(&path)?;
    file.write_all(contents.as_bytes())?;
    Ok(path)
}

impl Default for CommandHandler {
    fn default() -> Self {
        Self::new()
    }
}
