pub mod commands;
pub mod output;

use crate::cli::commands::CommandHandler;
use crate::core::types::*;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "sql-optimizer-cli")]
#[command(about = "Intelligent SQL query optimization advisor")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Verbose output
    #[arg(short, long)]
    pub verbose: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Analyze a single SQL query
    Analyze {
        /// SQL query to analyze
        query: String,

        /// Database connection string
        #[arg(short, long)]
        db: String,

        /// Show execution plan
        #[arg(long)]
        explain: bool,

        /// Output format (text, json, yaml)
        #[arg(short, long, default_value = "text")]
        output: OutputFormat,

        /// Force simple queries (avoid prepared statements, for pgbouncer transaction pooling)
        #[arg(long)]
        simple_mode: bool,

        /// Connection timeout in seconds (overrides default)
        #[arg(long)]
        connect_timeout: Option<u64>,
    },
    /// Interactive mode for multiple queries
    Interactive {
        /// Database connection string
        #[arg(short, long)]
        db: String,

        /// History file path
        #[arg(short, long, default_value = "~/.sql-optimizer-history")]
        history: std::path::PathBuf,

        /// Output format (text, json, yaml)
        #[arg(short, long, default_value = "text")]
        output: OutputFormat,

        /// Force simple queries (avoid prepared statements)
        #[arg(long)]
        simple_mode: bool,

        /// Connection timeout in seconds
        #[arg(long)]
        connect_timeout: Option<u64>,
    },
    /// Analyze multiple queries from file
    Batch {
        /// Database connection string
        #[arg(short, long)]
        db: String,

        /// Input file with queries
        #[arg(short, long)]
        input: std::path::PathBuf,

        /// Output file for recommendations
        #[arg(short, long)]
        output: std::path::PathBuf,

        /// Force simple queries (avoid prepared statements)
        #[arg(long)]
        simple_mode: bool,

        /// Connection timeout in seconds
        #[arg(long)]
        connect_timeout: Option<u64>,
    },
    /// Introspect and print database schema
    Schema {
        /// Database connection string
        #[arg(short, long)]
        db: String,

        /// Force simple queries (avoid prepared statements)
        #[arg(long)]
        simple_mode: bool,

        /// Connection timeout in seconds
        #[arg(long)]
        connect_timeout: Option<u64>,
    },
}

impl Cli {
    pub async fn execute(&self) -> anyhow::Result<()> {
        let handler = CommandHandler::new();

        match &self.command {
            Commands::Analyze { query, db, explain, output, simple_mode, connect_timeout } => {
                handler
                    .handle_analyze(query, db, *explain, output.clone(), self.verbose, *simple_mode, *connect_timeout)
                    .await
            }
            Commands::Interactive { db, history, output: _, simple_mode, connect_timeout } => {
                handler.handle_interactive(history, db, *simple_mode, *connect_timeout).await
            }
            Commands::Batch { db, input, output, simple_mode, connect_timeout } => {
                handler
                    .handle_batch(input, output, db, *simple_mode, *connect_timeout)
                    .await
            }
            Commands::Schema { db, simple_mode, connect_timeout } => {
                handler.handle_schema(db, *simple_mode, *connect_timeout).await
            }
        }
    }
}
