use clap::Parser;
use sql_optimizer_cli::cli::Cli;

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    let cli = Cli::parse_from(sql_optimizer_cli::cli::normalize_invocation(
        std::env::args_os(),
    ));

    // Distinct exit codes (Phase 7):
    //   0 = clean, 1 = findings below blocking threshold,
    //   2 = blocking findings (--fail-on exceeded), 3 = tool error.
    let code = match cli.execute().await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            3
        }
    };

    if code != 0 {
        std::process::exit(code);
    }
}
