use sql_optimizer_cli::core::types::{SchemaSnapshot, Severity};
use sql_optimizer_cli::security::injection::detect_injection_risks;
use sql_optimizer_cli::security::sensitive_data::detect_sensitive_data;
use sql_optimizer_cli::security::validator::{compute_security_score, validate_security};

#[test]
fn detects_string_concatenation_injection() {
    let query = "SELECT * FROM users WHERE name = 'admin' + @user_input";
    let issues = detect_injection_risks(query);
    assert!(
        issues
            .iter()
            .any(|i| i.description.contains("string concatenation")),
        "should detect string concatenation injection vector"
    );
}

#[test]
fn detects_stacked_queries() {
    let query = "SELECT * FROM users; DROP TABLE users;";
    let issues = detect_injection_risks(query);
    assert!(
        issues
            .iter()
            .any(|i| i.description.contains("Multiple SQL statements")),
        "should detect stacked queries"
    );
}

#[test]
fn detects_hardcoded_password() {
    let query = "SELECT * FROM users WHERE password = 'hunter2'";
    let issues = detect_injection_risks(query);
    assert!(
        issues
            .iter()
            .any(|i| i.description.contains("Hardcoded credential")),
        "should detect hardcoded password"
    );
}

#[test]
fn no_false_positive_on_clean_query() {
    let query = "SELECT id, name FROM users WHERE id = $1";
    let issues = detect_injection_risks(query);
    assert!(issues.is_empty(), "parameterized query should not trigger");
}

#[test]
fn detects_sensitive_columns() {
    let schema = SchemaSnapshot {
        tables: vec![sql_optimizer_cli::core::types::TableSchema {
            name: "users".to_string(),
            columns: vec![
                sql_optimizer_cli::core::types::ColumnInfo {
                    name: "id".to_string(),
                    data_type: "int".to_string(),
                },
                sql_optimizer_cli::core::types::ColumnInfo {
                    name: "password".to_string(),
                    data_type: "text".to_string(),
                },
                sql_optimizer_cli::core::types::ColumnInfo {
                    name: "email".to_string(),
                    data_type: "text".to_string(),
                },
            ],
            indexes: vec![],
            foreign_keys: vec![],
        }],
    };

    let query = "SELECT id, password, email FROM users";
    let issues = detect_sensitive_data(query, &schema);
    assert!(
        issues.iter().any(|i| i.description.contains("password")),
        "should detect password column"
    );
    assert!(
        issues.iter().any(|i| i.description.contains("email")),
        "should detect email column"
    );
}

#[test]
fn select_star_flags_sensitive_tables() {
    let schema = SchemaSnapshot {
        tables: vec![sql_optimizer_cli::core::types::TableSchema {
            name: "users".to_string(),
            columns: vec![
                sql_optimizer_cli::core::types::ColumnInfo {
                    name: "id".to_string(),
                    data_type: "int".to_string(),
                },
                sql_optimizer_cli::core::types::ColumnInfo {
                    name: "ssn".to_string(),
                    data_type: "text".to_string(),
                },
            ],
            indexes: vec![],
            foreign_keys: vec![],
        }],
    };

    let query = "SELECT * FROM users";
    let issues = detect_sensitive_data(query, &schema);
    assert!(
        issues.iter().any(|i| i.description.contains("SELECT *")),
        "should flag SELECT * on table with sensitive columns"
    );
}

#[test]
fn security_score_deductions() {
    let issues = vec![
        sql_optimizer_cli::core::types::SecurityIssue {
            issue_type: sql_optimizer_cli::core::types::SecurityIssueType::SqlInjection,
            description: "test".to_string(),
            severity: Severity::High,
            location: None,
        },
        sql_optimizer_cli::core::types::SecurityIssue {
            issue_type: sql_optimizer_cli::core::types::SecurityIssueType::SqlInjection,
            description: "test2".to_string(),
            severity: Severity::Medium,
            location: None,
        },
    ];
    let score = compute_security_score(&issues);
    // High = -30, Medium = -15 → 100 - 45 = 55
    assert!(
        (score - 55.0).abs() < 0.01,
        "score should be 55.0, got {}",
        score
    );
}

#[test]
fn full_validator_returns_issues_and_score() {
    let schema = SchemaSnapshot::default();
    let query = "SELECT * FROM users DROP TABLE users";
    let (score, issues) = validate_security(query, &schema);
    assert!(!issues.is_empty(), "should find security issues");
    assert!(score < 100.0, "score should be below 100");
}
