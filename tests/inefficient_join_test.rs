use sql_optimizer_cli::core::types::{
    ColumnInfo, ForeignKeyInfo, IndexInfo, SchemaSnapshot, TableSchema,
};
use sql_optimizer_cli::patterns::inefficient_join::detect_inefficient_joins;

#[test]
fn detects_join_on_unindexed_columns() {
    let schema = SchemaSnapshot {
        tables: vec![
            TableSchema {
                name: "users".to_string(),
                columns: vec![ColumnInfo {
                    name: "id".to_string(),
                    data_type: "int".to_string(),
                }],
                indexes: vec![],
                foreign_keys: vec![],
            },
            TableSchema {
                name: "orders".to_string(),
                columns: vec![
                    ColumnInfo {
                        name: "id".to_string(),
                        data_type: "int".to_string(),
                    },
                    ColumnInfo {
                        name: "user_id".to_string(),
                        data_type: "int".to_string(),
                    },
                ],
                indexes: vec![],
                foreign_keys: vec![],
            },
        ],
    };

    let query = "SELECT * FROM users JOIN orders ON users.id = orders.user_id";
    let recs = detect_inefficient_joins(query, &schema);
    assert!(!recs.is_empty(), "should detect unindexed join columns");
    assert!(recs
        .iter()
        .any(|r| r.description.contains("neither column is indexed")));
}

#[test]
fn no_false_positive_when_indexed() {
    let schema = SchemaSnapshot {
        tables: vec![
            TableSchema {
                name: "users".to_string(),
                columns: vec![ColumnInfo {
                    name: "id".to_string(),
                    data_type: "int".to_string(),
                }],
                indexes: vec![IndexInfo {
                    name: "idx_users_id".to_string(),
                    columns: vec!["id".to_string()],
                    is_unique: true,
                }],
                foreign_keys: vec![],
            },
            TableSchema {
                name: "orders".to_string(),
                columns: vec![
                    ColumnInfo {
                        name: "id".to_string(),
                        data_type: "int".to_string(),
                    },
                    ColumnInfo {
                        name: "user_id".to_string(),
                        data_type: "int".to_string(),
                    },
                ],
                indexes: vec![IndexInfo {
                    name: "idx_orders_user_id".to_string(),
                    columns: vec!["user_id".to_string()],
                    is_unique: false,
                }],
                foreign_keys: vec![],
            },
        ],
    };

    let query = "SELECT * FROM users JOIN orders ON users.id = orders.user_id";
    let recs = detect_inefficient_joins(query, &schema);
    assert!(recs.is_empty(), "indexed join columns should not trigger");
}

#[test]
fn detects_fk_without_index() {
    let schema = SchemaSnapshot {
        tables: vec![
            TableSchema {
                name: "users".to_string(),
                columns: vec![ColumnInfo {
                    name: "id".to_string(),
                    data_type: "int".to_string(),
                }],
                indexes: vec![IndexInfo {
                    name: "idx_users_id".to_string(),
                    columns: vec!["id".to_string()],
                    is_unique: true,
                }],
                foreign_keys: vec![],
            },
            TableSchema {
                name: "orders".to_string(),
                columns: vec![
                    ColumnInfo {
                        name: "id".to_string(),
                        data_type: "int".to_string(),
                    },
                    ColumnInfo {
                        name: "user_id".to_string(),
                        data_type: "int".to_string(),
                    },
                ],
                indexes: vec![],
                foreign_keys: vec![ForeignKeyInfo {
                    name: "fk_orders_user".to_string(),
                    columns: vec!["user_id".to_string()],
                    referenced_table: "users".to_string(),
                    referenced_columns: vec!["id".to_string()],
                }],
            },
        ],
    };

    let query = "SELECT * FROM users JOIN orders ON users.id = orders.user_id";
    let recs = detect_inefficient_joins(query, &schema);
    // Should detect that the FK column user_id isn't indexed
    assert!(
        recs.iter().any(|r| r.description.contains("Foreign key")),
        "should flag unindexed FK column"
    );
}
