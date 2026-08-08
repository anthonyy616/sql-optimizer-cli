use anyhow::Result;
use std::path::PathBuf;
use std::{fs, io::Write, path::Path};

use crate::cli::output::OutputFormatter;
use crate::cli::ConnectionArgs;
use crate::core::analyzer::SqlAnalyzer;
use crate::core::types::{DatabaseType, OutputFormat};
use crate::core::fingerprint::fingerprint;
use crate::database::connection::create_connector;

pub struct CommandHandler {
    analyzer: SqlAnalyzer,
}

impl CommandHandler {
    pub fn new() -> Self {
        Self {
            analyzer: SqlAnalyzer::new(),
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
    ) -> Result<()> {
        let db_url = connection.resolve_connection_string()?;

        if verbose {
            eprintln!(
                "Connecting to database: {}",
                &db_url[..std::cmp::min(db_url.len(), 20)]
            );
            eprintln!("Query: {}", query);
        }

        // Determine database type from URL
        let db_type = if db_url.starts_with("postgresql") || db_url.starts_with("postgres") {
            DatabaseType::PostgreSQL
        } else if db_url.starts_with("mysql") {
            DatabaseType::MySQL
        } else if db_url.starts_with("sqlite")
            || db_url.ends_with(".db")
            || db_url.ends_with(".sqlite")
        {
            DatabaseType::SQLite
        } else {
            return Err(anyhow::anyhow!("Unsupported database URL format. Must start with postgresql://, mysql://, or sqlite://"));
        };

        // Create and connect to database
        let mut connector = create_connector(db_type);
        let options = crate::core::types::ConnectOptions {
            simple_mode,
            connect_timeout_secs: connect_timeout,
            accept_invalid_certs: connection.accept_invalid_certs,
        };
        connector.connect(&db_url, &options).await?;

        // Create analyzer with database connection
        let analyzer_with_db = self.analyzer.with_database();

        // Perform analysis
        let mut result = analyzer_with_db.analyze_query(query, db_type).await?;

        let schema = connector.introspect_schema().await?;
        result.schema_snapshot = Some(schema);

        if show_rows {
            match connector.preview_rows(query, row_limit).await {
                Ok(preview) => result.row_preview = Some(preview),
                Err(e) => eprintln!("Warning: row preview unavailable: {}", e),
            }
        }

        // Run schema-dependent checks (missing index, etc.)
        let analyzer = crate::core::analyzer::SqlAnalyzer::new();
        analyzer.run_schema_checks(&mut result).await?;

        // Show execution plan if requested
        if explain {
            match connector.explain_query(query).await {
                Ok(plan) => result.explain_plan = Some(plan),
                Err(e) => eprintln!("Warning: explain unavailable: {}", e),
            }
        }

        // Format and output results
        let is_text = matches!(output_format, OutputFormat::Text);
        let formatter = OutputFormatter::new(output_format.clone());
        if is_text {
            formatter.format(&result)?;
        } else {
            let rendered = formatter.render(&result)?;
            let path = write_auto_output("analyze", query, &output_format, &rendered)?;
            println!("Results written to {}", path.display());
        }

        Ok(())
    }

    pub async fn handle_interactive(
        &self,
        history_file: &PathBuf,
        connection: &ConnectionArgs,
        show_rows: bool,
        row_limit: usize,
        output_format: OutputFormat,
        simple_mode: bool,
        connect_timeout: Option<u64>,
    ) -> Result<()> {
        use dialoguer::Input;
        use std::fs::OpenOptions;
        use std::io::Write;

        let db_url = connection.resolve_connection_string()?;

        // Determine database type
        let db_type = if db_url.starts_with("postgresql") || db_url.starts_with("postgres") {
            DatabaseType::PostgreSQL
        } else if db_url.starts_with("mysql") {
            DatabaseType::MySQL
        } else if db_url.starts_with("sqlite")
            || db_url.ends_with(".db")
            || db_url.ends_with(".sqlite")
        {
            DatabaseType::SQLite
        } else {
            return Err(anyhow::anyhow!("Unsupported database URL format"));
        };

        // Create and connect to database
        let mut connector = create_connector(db_type);
        let options = crate::core::types::ConnectOptions {
            simple_mode,
            connect_timeout_secs: connect_timeout,
            accept_invalid_certs: connection.accept_invalid_certs,
        };
        connector.connect(&db_url, &options).await?;

        // Create analyzer with database connection
        let analyzer_with_db = self.analyzer.with_database();

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
            match analyzer_with_db.analyze_query(&query, db_type).await {
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
                        let path = write_auto_output("interactive", &query, &output_format, &rendered)?;
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

        Ok(())
    }

    pub async fn handle_batch(
        &self,
        input_file: &PathBuf,
        output_file: &Option<PathBuf>,
        output_format: OutputFormat,
        connection: &ConnectionArgs,
        simple_mode: bool,
        connect_timeout: Option<u64>,
    ) -> Result<()> {
        let db_url = connection.resolve_connection_string()?;

        println!("Processing batch file: {:?}", input_file);

        // Determine database type
        let db_type = if db_url.starts_with("postgresql") || db_url.starts_with("postgres") {
            DatabaseType::PostgreSQL
        } else if db_url.starts_with("mysql") {
            DatabaseType::MySQL
        } else if db_url.starts_with("sqlite")
            || db_url.ends_with(".db")
            || db_url.ends_with(".sqlite")
        {
            DatabaseType::SQLite
        } else {
            return Err(anyhow::anyhow!("Unsupported database URL format"));
        };

        // Create and connect to database
        let mut connector = create_connector(db_type);
        let options = crate::core::types::ConnectOptions {
            simple_mode,
            connect_timeout_secs: connect_timeout,
            accept_invalid_certs: connection.accept_invalid_certs,
        };
        connector.connect(&db_url, &options).await?;

        // Create analyzer with database connection
        let analyzer_with_db = self.analyzer.with_database();

        let content = fs::read_to_string(input_file)?;
        let queries: Vec<&str> = content
            .lines()
            .filter(|line| !line.trim().is_empty() && !line.trim().starts_with("--"))
            .collect();

        println!("Found {} queries to analyze", queries.len());

        let mut results = Vec::new();

        for (i, query) in queries.iter().enumerate() {
            println!("Analyzing query {}/{}", i + 1, queries.len());

            match analyzer_with_db.analyze_query(query, db_type).await {
                Ok(result) => results.push(result),
                Err(e) => eprintln!("Error analyzing query {}: {}", i + 1, e),
            }
        }

        let json_output = serde_json::to_string_pretty(&results)?;
        if let Some(output_file) = output_file {
            fs::write(output_file, json_output)?;
            println!("Batch analysis complete. Results written to: {:?}", output_file);
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
                        rendered.push_str(&OutputFormatter::new(OutputFormat::Markdown).render(result)?);
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
        Ok(())
    }

    pub async fn handle_schema(
        &self,
        connection: &ConnectionArgs,
        simple_mode: bool,
        connect_timeout: Option<u64>,
    ) -> Result<()> {
        let db_url = connection.resolve_connection_string()?;

        // Determine database type from URL
        let db_type = if db_url.starts_with("postgresql") || db_url.starts_with("postgres") {
            DatabaseType::PostgreSQL
        } else if db_url.starts_with("mysql") {
            DatabaseType::MySQL
        } else if db_url.starts_with("sqlite")
            || db_url.ends_with(".db")
            || db_url.ends_with(".sqlite")
        {
            DatabaseType::SQLite
        } else {
            return Err(anyhow::anyhow!("Unsupported database URL format. Must start with postgresql://, mysql://, or sqlite://"));
        };

        let mut connector = create_connector(db_type);
        let options = crate::core::types::ConnectOptions {
            simple_mode,
            connect_timeout_secs: connect_timeout,
            accept_invalid_certs: connection.accept_invalid_certs,
        };
        connector.connect(&db_url, &options).await?;

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
        }

        Ok(())
    }
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
        OutputFormat::Text => return Err(anyhow::anyhow!("text output is not auto-written")),
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
