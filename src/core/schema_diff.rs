use crate::core::types::{SchemaDriftItem, SchemaSnapshot};

use std::collections::{HashMap, HashSet};

/// Diff two schema snapshots (old baseline vs. current live schema) and
/// report material changes. Reuses the snapshot machinery from Phase 1 —
/// no new database round-trips are required beyond introspection.
pub fn diff_schemas(old: &SchemaSnapshot, new: &SchemaSnapshot) -> Vec<SchemaDriftItem> {
    let mut drift = Vec::new();

    let old_tables: HashMap<&str, &_> = old.tables.iter().map(|t| (t.name.as_str(), t)).collect();
    let new_tables: HashMap<&str, &_> = new.tables.iter().map(|t| (t.name.as_str(), t)).collect();

    // Dropped tables
    for name in old_tables.keys() {
        if !new_tables.contains_key(name) {
            drift.push(SchemaDriftItem {
                kind: "table-dropped".to_string(),
                table: name.to_string(),
                detail: format!("Table '{}' existed in the baseline but is gone", name),
            });
        }
    }

    // Added tables
    for name in new_tables.keys() {
        if !old_tables.contains_key(name) {
            drift.push(SchemaDriftItem {
                kind: "table-added".to_string(),
                table: name.to_string(),
                detail: format!("Table '{}' is new since the baseline", name),
            });
        }
    }

    // Tables present in both: diff columns and indexes
    for (name, old_t) in old_tables.iter() {
        let new_t = match new_tables.get(name) {
            Some(t) => t,
            None => continue,
        };

        let old_cols: HashMap<&str, &str> = old_t
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c.data_type.as_str()))
            .collect();
        let new_cols: HashMap<&str, &str> = new_t
            .columns
            .iter()
            .map(|c| (c.name.as_str(), c.data_type.as_str()))
            .collect();

        for (col, ty) in old_cols.iter() {
            match new_cols.get(col) {
                None => drift.push(SchemaDriftItem {
                    kind: "column-dropped".to_string(),
                    table: name.to_string(),
                    detail: format!("Column '{}.{}' ({}) was dropped", name, col, ty),
                }),
                Some(new_ty) if new_ty != ty => drift.push(SchemaDriftItem {
                    kind: "column-type-changed".to_string(),
                    table: name.to_string(),
                    detail: format!(
                        "Column '{}.{}' changed type {} -> {}",
                        name, col, ty, new_ty
                    ),
                }),
                _ => {}
            }
        }
        for col in new_cols.keys() {
            if !old_cols.contains_key(col) {
                drift.push(SchemaDriftItem {
                    kind: "column-added".to_string(),
                    table: name.to_string(),
                    detail: format!("Column '{}.{}' is new since the baseline", name, col),
                });
            }
        }

        let old_idx: HashSet<&str> = old_t.indexes.iter().map(|i| i.name.as_str()).collect();
        let new_idx: HashSet<&str> = new_t.indexes.iter().map(|i| i.name.as_str()).collect();

        for idx in old_idx.difference(&new_idx) {
            drift.push(SchemaDriftItem {
                kind: "index-dropped".to_string(),
                table: name.to_string(),
                detail: format!(
                    "Index '{}' on '{}' was dropped since the baseline",
                    idx, name
                ),
            });
        }
        for idx in new_idx.difference(&old_idx) {
            drift.push(SchemaDriftItem {
                kind: "index-added".to_string(),
                table: name.to_string(),
                detail: format!("Index '{}' on '{}' is new since the baseline", idx, name),
            });
        }
    }

    drift.sort_by(|a, b| (&a.kind, &a.table).cmp(&(&b.kind, &b.table)));
    drift
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{ColumnInfo, IndexInfo, TableSchema};

    fn snapshot(tables: Vec<TableSchema>) -> SchemaSnapshot {
        SchemaSnapshot { tables }
    }

    #[test]
    fn detects_dropped_index_and_type_change() {
        let old = snapshot(vec![TableSchema {
            name: "users".into(),
            columns: vec![
                ColumnInfo {
                    name: "id".into(),
                    data_type: "int4".into(),
                },
                ColumnInfo {
                    name: "email".into(),
                    data_type: "varchar".into(),
                },
            ],
            indexes: vec![IndexInfo {
                name: "idx_users_email".into(),
                columns: vec!["email".into()],
                is_unique: false,
            }],
            foreign_keys: vec![],
        }]);

        let new = snapshot(vec![TableSchema {
            name: "users".into(),
            columns: vec![
                ColumnInfo {
                    name: "id".into(),
                    data_type: "int8".into(), // type changed
                },
                ColumnInfo {
                    name: "email".into(),
                    data_type: "varchar".into(),
                },
            ],
            indexes: vec![], // index dropped
            foreign_keys: vec![],
        }]);

        let drift = diff_schemas(&old, &new);
        let kinds: Vec<&str> = drift.iter().map(|d| d.kind.as_str()).collect();
        assert!(kinds.contains(&"index-dropped"));
        assert!(kinds.contains(&"column-type-changed"));

        let dropped = drift.iter().find(|d| d.kind == "index-dropped").unwrap();
        assert!(dropped.detail.contains("idx_users_email"));
    }

    #[test]
    fn identical_snapshots_produce_no_drift() {
        let snap = snapshot(vec![TableSchema {
            name: "t".into(),
            columns: vec![ColumnInfo {
                name: "a".into(),
                data_type: "text".into(),
            }],
            indexes: vec![],
            foreign_keys: vec![],
        }]);
        assert!(diff_schemas(&snap, &snap).is_empty());
    }
}
