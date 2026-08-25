pub mod commands;
pub mod output;

use crate::cli::commands::CommandHandler;
use crate::core::types::*;
use anyhow::{anyhow, Result};
use clap::{Args, Parser, Subcommand};
use std::ffi::{OsStr, OsString};
use std::path::Path;

const COMMAND_SHORTCUTS: &[&str] = &["analyze", "batch", "interactive", "schema"];

pub fn normalize_invocation<I>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args: Vec<OsString> = args.into_iter().collect();
    if args.len() < 2 {
        return args;
    }

    let program_name = args
        .first()
        .and_then(|path| Path::new(path).file_stem())
        .and_then(OsStr::to_str)
        .unwrap_or_default();

    if !COMMAND_SHORTCUTS.contains(&program_name) {
        return args;
    }

    let first_argument = args
        .get(1)
        .and_then(|value| value.to_str())
        .unwrap_or_default();

    if first_argument == program_name {
        return args;
    }

    args.insert(1, OsString::from(program_name));
    args
}

#[derive(Debug, Clone, Args, Default)]
pub struct ConnectionArgs {
    /// Database connection string
    #[arg(short, long, env = "SQL_OPTIMIZER_DB_URL")]
    pub db: Option<String>,

    /// Database host used when building a connection string from parts
    #[arg(long, env = "SQL_OPTIMIZER_DB_HOST")]
    pub db_host: Option<String>,

    /// Database port used when building a connection string from parts
    #[arg(long, env = "SQL_OPTIMIZER_DB_PORT")]
    pub db_port: Option<u16>,

    /// Database user used when building a connection string from parts
    #[arg(long, env = "SQL_OPTIMIZER_DB_USER")]
    pub db_user: Option<String>,

    /// Database password used when building a connection string from parts
    #[arg(long, env = "SQL_OPTIMIZER_DB_PASSWORD")]
    pub db_password: Option<String>,

    /// Database name used when building a connection string from parts
    #[arg(long, env = "SQL_OPTIMIZER_DB_NAME")]
    pub db_name: Option<String>,

    /// SSL mode used when building a PostgreSQL connection string from parts
    #[arg(long, env = "SQL_OPTIMIZER_DB_SSLMODE", default_value = "require")]
    pub db_sslmode: String,

    /// Allow self-signed or otherwise invalid certificates when building a PostgreSQL connection string
    #[arg(long, env = "SQL_OPTIMIZER_DB_ACCEPT_INVALID_CERTS")]
    pub accept_invalid_certs: bool,
}

impl ConnectionArgs {
    pub fn resolve_connection_string(&self) -> Result<String> {
        if let Some(db) = self.db.as_ref() {
            return Ok(db.clone());
        }

        let host = self.db_host.as_ref().ok_or_else(|| {
            anyhow!("Missing database host. Provide --db or set SQL_OPTIMIZER_DB_HOST.")
        })?;
        let user = self.db_user.as_ref().ok_or_else(|| {
            anyhow!("Missing database user. Provide --db or set SQL_OPTIMIZER_DB_USER.")
        })?;
        let password = self.db_password.as_ref().ok_or_else(|| {
            anyhow!("Missing database password. Provide --db or set SQL_OPTIMIZER_DB_PASSWORD.")
        })?;
        let name = self.db_name.as_ref().ok_or_else(|| {
            anyhow!("Missing database name. Provide --db or set SQL_OPTIMIZER_DB_NAME.")
        })?;

        let port = self.db_port.unwrap_or(5432);
        let sslmode = if self.db_sslmode.trim().is_empty() {
            "require"
        } else {
            self.db_sslmode.as_str()
        };

        let encoded_user = urlencoding::encode(user);
        let encoded_password = urlencoding::encode(password);
        let encoded_name = urlencoding::encode(name);
        let formatted_host = if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]")
        } else {
            host.clone()
        };

        Ok(format!(
            "postgresql://{}:{}@{}:{}/{}?sslmode={}",
            encoded_user, encoded_password, formatted_host, port, encoded_name, sslmode
        ))
    }
}

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

    /// Analysis profile: oltp (backend/app queries) or analytics (data engineering/pipeline queries)
    #[arg(long, value_enum, default_value_t, global = true)]
    pub profile: crate::core::types::Profile,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Analyze a single SQL query
    Analyze {
        /// SQL query to analyze
        query: String,

        #[command(flatten)]
        connection: ConnectionArgs,

        /// Show execution plan
        #[arg(long)]
        explain: bool,

        /// Show a preview of matching rows
        #[arg(long)]
        show_rows: bool,

        /// Row preview limit
        #[arg(long, default_value = "50")]
        row_limit: usize,

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
        #[command(flatten)]
        connection: ConnectionArgs,

        /// History file path
        #[arg(short, long, default_value = "~/.sql-optimizer-history")]
        history: std::path::PathBuf,

        /// Show a preview of matching rows
        #[arg(long)]
        show_rows: bool,

        /// Row preview limit
        #[arg(long, default_value = "50")]
        row_limit: usize,

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
        #[command(flatten)]
        connection: ConnectionArgs,

        /// Input file with queries
        #[arg(short, long)]
        input: std::path::PathBuf,

        /// Output file for recommendations
        #[arg(long)]
        output_file: Option<std::path::PathBuf>,

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
    /// Introspect and print database schema
    Schema {
        #[command(flatten)]
        connection: ConnectionArgs,

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
            Commands::Analyze {
                query,
                connection,
                explain,
                show_rows,
                row_limit,
                output,
                simple_mode,
                connect_timeout,
            } => {
                handler
                    .handle_analyze(
                        query,
                        connection,
                        *explain,
                        *show_rows,
                        *row_limit,
                        output.clone(),
                        self.verbose,
                        *simple_mode,
                        *connect_timeout,
                        self.profile.clone(),
                    )
                    .await
            }
            Commands::Interactive {
                connection,
                history,
                show_rows,
                row_limit,
                output,
                simple_mode,
                connect_timeout,
            } => {
                handler
                    .handle_interactive(
                        history,
                        connection,
                        *show_rows,
                        *row_limit,
                        output.clone(),
                        *simple_mode,
                        *connect_timeout,
                        self.profile.clone(),
                    )
                    .await
            }
            Commands::Batch {
                connection,
                input,
                output_file,
                output,
                simple_mode,
                connect_timeout,
            } => {
                handler
                    .handle_batch(
                        input,
                        output_file,
                        output.clone(),
                        connection,
                        *simple_mode,
                        *connect_timeout,
                        self.profile.clone(),
                    )
                    .await
            }
            Commands::Schema {
                connection,
                simple_mode,
                connect_timeout,
            } => {
                handler
                    .handle_schema(connection, *simple_mode, *connect_timeout)
                    .await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_invocation;
    use std::ffi::OsString;

    #[test]
    fn prepends_shortcut_program_name_as_subcommand() {
        let args = vec![
            OsString::from("analyze"),
            OsString::from("SELECT 1"),
            OsString::from("--db"),
            OsString::from("sqlite::memory:"),
        ];

        let normalized = normalize_invocation(args);

        assert_eq!(normalized[1], OsString::from("analyze"));
        assert_eq!(normalized[2], OsString::from("SELECT 1"));
    }

    #[test]
    fn leaves_normal_binary_invocation_unchanged() {
        let args = vec![
            OsString::from("sql-optimizer-cli"),
            OsString::from("analyze"),
            OsString::from("SELECT 1"),
        ];

        let normalized = normalize_invocation(args.clone());

        assert_eq!(normalized, args);
    }
}
