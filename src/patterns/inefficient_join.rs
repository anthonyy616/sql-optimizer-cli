use crate::core::types::{ConfidenceTier, Recommendation, RecommendationType, SchemaSnapshot};
use sqlparser::ast::{JoinConstraint, JoinOperator, SetExpr, Statement, TableFactor};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;

/// Detect inefficient JOIN patterns:
/// - JOIN on columns that have no index
/// - JOINs that could leverage existing foreign keys but don't reference them
pub fn detect_inefficient_joins(query: &str, schema: &SchemaSnapshot) -> Vec<Recommendation> {
    let mut recs = Vec::new();

    let dialect = GenericDialect {};
    let statements = match Parser::parse_sql(&dialect, query) {
        Ok(s) => s,
        Err(_) => return recs,
    };

    for stmt in &statements {
        if let Statement::Query(query_box) = stmt {
            if let SetExpr::Select(select) = &*query_box.body {
                if select.from.is_empty() {
                    continue;
                }
                for join in &select.from[0].joins {
                    // Extract the ON constraint from the join operator
                    let constraint = match &join.join_operator {
                        JoinOperator::Inner(c)
                        | JoinOperator::LeftOuter(c)
                        | JoinOperator::RightOuter(c)
                        | JoinOperator::FullOuter(c) => c,
                        _ => continue,
                    };

                    let on_expr = match constraint {
                        JoinConstraint::On(expr) => expr,
                        _ => continue,
                    };

                    // Extract the column names from the ON expression
                    let join_cols = extract_join_columns(on_expr);
                    if join_cols.0.is_empty() || join_cols.1.is_empty() {
                        continue;
                    }

                    // Get the table being joined
                    let joined_table = match &join.relation {
                        TableFactor::Table { name, .. } => name.0.last().map(|n| n.value.clone()),
                        _ => None,
                    };

                    let from_table = match &select.from[0].relation {
                        TableFactor::Table { name, .. } => name.0.last().map(|n| n.value.clone()),
                        _ => None,
                    };

                    // Check if any FK relationship exists between these tables
                    let has_fk = if let Some(ref_table) = &joined_table {
                        let ref_col = &join_cols.1;
                        schema.tables.iter().any(|t| {
                            t.foreign_keys.iter().any(|fk| {
                                (fk.referenced_table == *ref_table || t.name == *ref_table)
                                    && fk.columns.iter().any(|c| c == ref_col)
                            })
                        })
                    } else {
                        false
                    };

                    // Check if the join columns are indexed
                    let left_indexed = if let Some(ref tbl) = from_table {
                        schema.tables.iter().any(|t| {
                            t.name == *tbl
                                && t.indexes
                                    .iter()
                                    .any(|idx| idx.columns.iter().any(|c| c == &join_cols.0))
                        })
                    } else {
                        false
                    };

                    let right_indexed = if let Some(ref tbl) = joined_table {
                        schema.tables.iter().any(|t| {
                            t.name == *tbl
                                && t.indexes
                                    .iter()
                                    .any(|idx| idx.columns.iter().any(|c| c == &join_cols.1))
                        })
                    } else {
                        false
                    };

                    if !left_indexed && !right_indexed {
                        let tbl = from_table.as_deref().unwrap_or("table");
                        let mut suggestion = format!(
                            "Add an index on the join columns, e.g. CREATE INDEX idx_{}_{} ON {}({});",
                            tbl, join_cols.0, tbl, join_cols.0,
                        );
                        if has_fk {
                            suggestion.push_str(
                                " Note: a foreign key relationship exists between these tables \
                                 — an index on the referencing column is expected.",
                            );
                        }

                        recs.push(Recommendation {
                            recommendation_type: RecommendationType::InefficientJoin,
                            table: from_table.clone(),
                            columns: vec![join_cols.0.clone(), join_cols.1.clone()],
                            description: format!(
                                "JOIN on '{}' and '{}' — neither column is indexed, causing a nested loop or hash join on unindexed columns",
                                join_cols.0, join_cols.1
                            ),
                            estimated_improvement: 0.6,
                            sql_suggestion: Some(suggestion),
                            confidence: ConfidenceTier::SchemaVerified,
                        });
                    } else if has_fk && !right_indexed {
                        // FK exists but the foreign key column isn't indexed
                        let tbl = joined_table.as_deref().unwrap_or("table");
                        recs.push(Recommendation {
                            recommendation_type: RecommendationType::InefficientJoin,
                            table: joined_table.clone(),
                            columns: vec![join_cols.1.clone()],
                            description: format!(
                                "Foreign key column '{}' on table '{}' is not indexed — JOINs through this FK will be slow",
                                join_cols.1, tbl
                            ),
                            estimated_improvement: 0.5,
                            sql_suggestion: Some(format!(
                                "CREATE INDEX idx_{}_{} ON {}({});",
                                tbl, join_cols.1, tbl, join_cols.1,
                            )),
                            confidence: ConfidenceTier::SchemaVerified,
                        });
                    }
                }
            }
        }
    }

    recs
}

/// Extract (left_column, right_column) from a JOIN ON expression.
fn extract_join_columns(on_expr: &sqlparser::ast::Expr) -> (String, String) {
    use sqlparser::ast::{BinaryOperator, Expr};

    match on_expr {
        Expr::BinaryOp {
            left,
            op: BinaryOperator::Eq,
            right,
        } => {
            let left_col = extract_column_name(left);
            let right_col = extract_column_name(right);
            (left_col, right_col)
        }
        Expr::BinaryOp {
            left,
            op: BinaryOperator::And,
            right,
        } => {
            let (l, r) = extract_join_columns(left);
            if !l.is_empty() && !r.is_empty() {
                (l, r)
            } else {
                extract_join_columns(right)
            }
        }
        _ => (String::new(), String::new()),
    }
}

fn extract_column_name(expr: &sqlparser::ast::Expr) -> String {
    use sqlparser::ast::Expr;
    match expr {
        Expr::Identifier(ident) => ident.value.clone(),
        Expr::CompoundIdentifier(idents) => {
            idents.last().map(|i| i.value.clone()).unwrap_or_default()
        }
        _ => String::new(),
    }
}
