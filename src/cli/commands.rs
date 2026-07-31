use anyhow::Result;
use std::path::PathBuf;

use crate::cli::output::OutputFormatter;
use crate::cli::ConnectionArgs;
use crate::core::analyzer::SqlAnalyzer;
use crate::core::types::{DatabaseType, OutputFormat};
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

        // Run schema-dependent checks (missing index, etc.)
        let analyzer = crate::core::analyzer::SqlAnalyzer::new();
        analyzer.run_schema_checks(&mut result).await?;

        // Show execution plan if requested
        if explain {
            let plan = connector.explain_query(query).await?;
            result.explain_plan = Some(plan);
        }

        // Format and output results
        let formatter = OutputFormatter::new(output_format);
        formatter.format(&result)?;

        Ok(())
    }

    pub async fn handle_interactive(
        &self,
        history_file: &PathBuf,
        connection: &ConnectionArgs,
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
                    let formatter = OutputFormatter::new(OutputFormat::Text);
                    formatter.format(&result)?;
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
        output_file: &PathBuf,
        connection: &ConnectionArgs,
        simple_mode: bool,
        connect_timeout: Option<u64>,
    ) -> Result<()> {
        use std::fs;

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

        // Write results to output file
        let json_output = serde_json::to_string_pretty(&results)?;
        fs::write(output_file, json_output)?;

        println!(
            "Batch analysis complete. Results written to: {:?}",
            output_file
        );
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

impl Default for CommandHandler {
    fn default() -> Self {
        Self::new()
    }
}
