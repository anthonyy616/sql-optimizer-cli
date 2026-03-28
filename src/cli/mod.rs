pub mod commands;
pub mod output;

use clap::{Parser, Subcommand};
use crate::core::types::*;
use crate::cli::commands::CommandHandler;

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
    },
}

impl Cli {
    pub async fn execute(&self) -> anyhow::Result<()> {
        let handler = CommandHandler::new();
        
        match &self.command {
            Commands::Analyze { query, db, explain, output } => {
                handler.handle_analyze(query, db, *explain, output.clone(), self.verbose).await
            }
            Commands::Interactive { db, history, output: _ } => {
                handler.handle_interactive(history, db).await
            }
            Commands::Batch { db, input, output } => {
                handler.handle_batch(input, output, db).await
            }
        }
    }
}
