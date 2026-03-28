use anyhow::Result;
use std::path::PathBuf;

use crate::core::analyzer::SqlAnalyzer;
use crate::core::types::DatabaseType;
use crate::cli::output::OutputFormatter;

pub struct CommandHandler {
    analyzer: SqlAnalyzer,
}

impl CommandHandler {
    pub fn new() -> Self {
        Self {
            analyzer: SqlAnalyzer::new(),
        }
    }

    pub async fn handle_analyze(&self, query: &str, db_url: &str, explain: bool, output_format: crate::core::types::OutputFormat, verbose: bool) -> Result<()> {
        if verbose {
            eprintln!("Connecting to database: {}", &db_url[..std::cmp::min(db_url.len(), 20)]);
            eprintln!("Query: {}", query);
        }

        // Determine database type from URL
        let db_type = if db_url.starts_with("postgresql") || db_url.starts_with("postgres") {
            DatabaseType::PostgreSQL
        } else if db_url.starts_with("mysql") {
            DatabaseType::MySQL
        } else {
            return Err(anyhow::anyhow!("Unsupported database URL format. Must start with postgresql:// or mysql://"));
        };

        // Perform analysis
        let result = self.analyzer.analyze_query(query, db_type)?;

        // Show execution plan if requested
        if explain {
            eprintln!("\n=== Execution Plan ===");
            eprintln!("(Execution plan analysis will be implemented with database connection)");
            eprintln!("===================\n");
        }

        // Format and output results
        let formatter = OutputFormatter::new(output_format);
        formatter.format(&result)?;

        Ok(())
    }

    pub async fn handle_interactive(&self, history_file: &PathBuf, db_url: &str) -> Result<()> {
        use dialoguer::Input;
        use std::fs::OpenOptions;
        use std::io::Write;

        println!("SQL Optimizer Interactive Mode");
        println!("Connected to: {}", &db_url[..std::cmp::min(db_url.len(), 20)]);
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
            match self.handle_analyze(&query, db_url, false, crate::core::types::OutputFormat::Text, false).await {
                Ok(_) => {
                    println!(); // Add spacing between results
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                }
            }
        }

        // Save history
        if let Ok(mut file) = OpenOptions::new().create(true).write(true).truncate(true).open(history_file) {
            for line in history.iter().rev().take(100) { // Keep last 100 queries
                writeln!(file, "{}", line)?;
            }
        }

        Ok(())
    }

    pub async fn handle_batch(&self, input_file: &PathBuf, output_file: &PathBuf, db_url: &str) -> Result<()> {
        use std::fs;

        println!("Processing batch file: {:?}", input_file);
        
        let content = fs::read_to_string(input_file)?;
        let queries: Vec<&str> = content.lines()
            .filter(|line| !line.trim().is_empty() && !line.trim().starts_with("--"))
            .collect();

        println!("Found {} queries to analyze", queries.len());

        let mut results = Vec::new();
        
        for (i, query) in queries.iter().enumerate() {
            println!("Analyzing query {}/{}", i + 1, queries.len());
            
            // Determine database type from URL
            let db_type = if db_url.starts_with("postgresql") || db_url.starts_with("postgres") {
                DatabaseType::PostgreSQL
            } else if db_url.starts_with("mysql") {
                DatabaseType::MySQL
            } else {
                return Err(anyhow::anyhow!("Unsupported database URL format"));
            };

            match self.analyzer.analyze_query(query, db_type) {
                Ok(result) => results.push(result),
                Err(e) => eprintln!("Error analyzing query {}: {}", i + 1, e),
            }
        }

        // Write results to output file
        let json_output = serde_json::to_string_pretty(&results)?;
        fs::write(output_file, json_output)?;

        println!("Batch analysis complete. Results written to: {:?}", output_file);
        Ok(())
    }
}
