use sql_optimizer_cli::cli::commands::CommandHandler;
use sql_optimizer_cli::core::types::OutputFormat;

#[tokio::test]
async fn analyze_runs_end_to_end_for_sqlite() {
    let db_path = tempfile::NamedTempFile::new()
        .expect("should create temp sqlite file")
        .into_temp_path();
    let db_url = format!("sqlite://{}", db_path.to_string_lossy());

    let handler = CommandHandler::new();
    let query = "SELECT * FROM users";
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

    let result = handler
        .handle_analyze(query, &db_url, true, OutputFormat::Json, false)
        .await;

    assert!(result.is_ok(), "analyze command should succeed: {result:?}");
}
