use sql_optimizer_cli::database::connection::create_connector;
use sql_optimizer_cli::core::types::DatabaseType;
use tempfile::NamedTempFile;

#[tokio::test]
async fn sqlite_schema_introspection() {
    let tmp = NamedTempFile::new().expect("tempfile");
    let path = tmp.into_temp_path();
    let db_file = path.to_string_lossy().to_string();
    let db_url = format!("sqlite://{}", db_file);

    // Create a table in the sqlite file so introspection returns a table
    {
        let conn = rusqlite::Connection::open(&db_file).expect("open sqlite");
        conn.execute(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT);",
            [],
        )
        .expect("create table");
    }

    let mut connector = create_connector(DatabaseType::SQLite);
    connector.connect(&db_url).await.expect("connect");
    let schema = connector.introspect_schema().await.expect("introspect");

    assert!(schema.tables.iter().any(|t| t.name == "users"));

    connector.disconnect().await.expect("disconnect");
}
