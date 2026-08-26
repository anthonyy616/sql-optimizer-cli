use sql_optimizer_cli::cli::commands::{CiOptions, CommandHandler};
use sql_optimizer_cli::cli::ConnectionArgs;
use sql_optimizer_cli::core::types::{OutputFormat, Profile};

#[tokio::test]
async fn analyze_runs_end_to_end_for_sqlite() {
    let db_path = tempfile::NamedTempFile::new()
        .expect("should create temp sqlite file")
        .into_temp_path();
    let db_url = format!("sqlite://{}", db_path.to_string_lossy());

    // Create table so analyze has something to query against
    let tmp_path = db_path.to_string_lossy().to_string();
    {
        let conn = rusqlite::Connection::open(&tmp_path).expect("open sqlite");
        conn.execute(
            "CREATE TABLE IF NOT EXISTS users (id INTEGER PRIMARY KEY, email TEXT);",
            [],
        )
        .expect("create table");
    }

    let handler = CommandHandler::new();
    let query = "SELECT * FROM users";
    let connection = ConnectionArgs {
        db: Some(db_url),
        ..Default::default()
    };

    let result = handler
        .handle_analyze(
            query,
            &connection,
            false, // explain
            false, // show_rows
            50,    // row_limit
            OutputFormat::Text,
            false, // verbose
            false, // simple_mode
            None,  // connect_timeout
            Profile::Oltp,
            false, // track
            None,  // schema_baseline
            CiOptions::default(),
        )
        .await;

    assert!(result.is_ok(), "analyze command should succeed: {result:?}");
}
