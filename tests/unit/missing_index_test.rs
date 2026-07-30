use sql_optimizer_cli::core::analyzer::SqlAnalyzer;
use sql_optimizer_cli::core::types::DatabaseType;

#[tokio::test]
async fn flags_missing_index_on_where() {
    let analyzer = SqlAnalyzer::new();
    let query = "SELECT id FROM users WHERE email = 'x@example.com'";
    let result = analyzer
        .analyze_query(query, DatabaseType::PostgreSQL)
        .await
        .expect("analysis should succeed");

    assert!(
        result
            .recommendations
            .iter()
            .any(|rec| rec.description.to_lowercase().contains("missing index")),
        "expected missing index recommendation"
    );
}
