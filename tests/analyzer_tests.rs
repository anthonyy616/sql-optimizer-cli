use sql_optimizer_cli::core::analyzer::SqlAnalyzer;
use sql_optimizer_cli::core::types::{DatabaseType, Profile};

#[tokio::test]
async fn flags_select_star_without_where() {
    let analyzer = SqlAnalyzer::new();
    let result = analyzer
        .analyze_query(
            "SELECT * FROM users",
            DatabaseType::PostgreSQL,
            Profile::Oltp,
        )
        .await
        .expect("analysis should succeed");

    assert!(result
        .recommendations
        .iter()
        .any(|rec| rec.description.contains("SELECT * without WHERE")));
}

#[tokio::test]
async fn flags_in_subquery_as_n_plus_one_pattern() {
    let analyzer = SqlAnalyzer::new();
    let query = "SELECT id FROM users WHERE id IN (SELECT user_id FROM orders)";
    let result = analyzer
        .analyze_query(query, DatabaseType::PostgreSQL, Profile::Oltp)
        .await
        .expect("analysis should succeed");

    assert!(result
        .recommendations
        .iter()
        .any(|rec| rec.description.contains("IN subquery detected")));
}

#[tokio::test]
async fn flags_basic_security_keywords() {
    let analyzer = SqlAnalyzer::new();
    let result = analyzer
        .analyze_query(
            "SELECT * FROM users UNION SELECT password FROM admins",
            DatabaseType::MySQL,
            Profile::Oltp,
        )
        .await
        .expect("analysis should succeed");

    assert!(!result.security_issues.is_empty());
    assert!(result
        .security_issues
        .iter()
        .any(|issue| issue.description.contains("union select")));
}
