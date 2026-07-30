use sql_optimizer_cli::core::types::{ColumnInfo, RecommendationType, SchemaSnapshot, TableSchema};
use sql_optimizer_cli::patterns::missing_index::detect_missing_index;

#[test]
fn detects_missing_index_for_where_column() {
    let schema = SchemaSnapshot {
        tables: vec![TableSchema {
            name: "users".to_string(),
            columns: vec![
                ColumnInfo {
                    name: "id".to_string(),
                    data_type: "int".to_string(),
                },
                ColumnInfo {
                    name: "email".to_string(),
                    data_type: "text".to_string(),
                },
            ],
            indexes: vec![],
            foreign_keys: vec![],
        }],
    };

    let query = "SELECT id FROM users WHERE email = 'x@example.com'";
    let recs = detect_missing_index(query, &schema);
    assert!(recs
        .iter()
        .any(|r| matches!(r.recommendation_type, RecommendationType::MissingIndex)));
}
