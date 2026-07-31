use anyhow::Result;
use clap::Parser;
use sql_optimizer_cli::cli::Cli;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();
    sql_optimizer_cli::run(cli).await
}
