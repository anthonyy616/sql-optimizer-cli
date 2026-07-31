use anyhow::Result;
use clap::Parser;
use sql_optimizer_cli::cli::Cli;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse_from(sql_optimizer_cli::cli::normalize_invocation(
        std::env::args_os(),
    ));
    sql_optimizer_cli::run(cli).await
}
