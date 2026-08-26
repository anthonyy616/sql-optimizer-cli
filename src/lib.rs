pub mod cli;
pub mod core;
pub mod database;
pub mod patterns;
pub mod rewriting;
pub mod scan;
pub mod security;
pub mod utils;

use anyhow::Result;
use cli::Cli;

pub async fn run(cli: Cli) -> Result<i32> {
    cli.execute().await
}
