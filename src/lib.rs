pub mod cli;
pub mod core;
pub mod database;
pub mod patterns;
pub mod security;
pub mod rewriting;
pub mod utils;

use anyhow::Result;
use cli::Cli;

pub async fn run(cli: Cli) -> Result<()> {
    cli.execute().await
}
