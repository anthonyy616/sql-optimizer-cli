use crate::core::types::{ConfidenceTier, Recommendation, RecommendationType, SchemaSnapshot};
use sqlparser::ast::{SetExpr, Statement, TableFactor};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;

/// Detect N+1 query anti-patterns:
/// - IN subqueries that could be JOINs
/// - Correlated subqueries that run once per row
/// - Queries missing a JOIN that a foreign key relationship suggests
pub fn detect_n_plus_one(query: &str, schema: &SchemaSnapshot) -> Vec<Recommendation> {
    let mut recs = Vec::new();

    let dialect = GenericDialect {};
    let statements = match Parser::parse_sql(&dialect, query) {
        Ok(s) => s,
        Err(_) => return recs,
    };

    for stmt in &statements {
        if let Statement::Query(query_box) = stmt {
            if let SetExpr::Select(select) = &*query_box.body {
                // Check for IN subqueries
                if let Some(where_clause) = &select.selection {
                    check_in_subqueries(where_clause, &mut recs);
                    check_correlated_subqueries(where_clause, &mut recs);
                }

                // Check for missing JOINs given FK relationships
                check_missing_joins(select, schema, &mut recs);
            }
        }
    }

    recs
}

fn check_in_subqueries(expr: &sqlparser::ast::Expr, recs: &mut Vec<Recommendation>) {
    use sqlparser::ast::Expr;

    match expr {
        Expr::InSubquery { subquery, .. } => {
            // Check if the subquery is simple enough to be rewritten as a JOIN
            let subquery_str = subquery.to_string();
            let is_simple = !subquery_str.contains("UNION")
                && !subquery_str.contains("GROUP BY")
                && !subquery_str.contains("HAVING");

            if is_simple {
                recs.push(Recommendation {
                    recommendation_type: RecommendationType::NPlusOneQuery,
                    table: None,
                    columns: vec![],
                    description: "IN subquery detected — consider rewriting as a JOIN for better performance on large datasets".to_string(),
                    estimated_improvement: 0.5,
                    sql_suggestion: Some("Replace IN (subquery) with INNER JOIN or EXISTS".to_string()),
                    confidence: ConfidenceTier::SyntacticGuess,
                });
            }
        }
        Expr::BinaryOp { left, right, .. } => {
            check_in_subqueries(left, recs);
            check_in_subqueries(right, recs);
        }
        Expr::Nested(inner) => {
            check_in_subqueries(inner, recs);
        }
        _ => {}
    }
}

fn check_correlated_subqueries(expr: &sqlparser::ast::Expr, recs: &mut Vec<Recommendation>) {
    use sqlparser::ast::Expr;

    match expr {
        Expr::Subquery(subquery) => {
            let subquery_str = subquery.to_string().to_lowercase();
            // A correlated subquery references a table from the outer query.
            // Heuristic: if the subquery contains a WHERE clause referencing a column
            // from the outer table, it's likely correlated.
            // We detect this by looking for common patterns like `outer_table.column =`
            // inside the subquery.
            if subquery_str.contains("where") && subquery_str.contains("= ") {
                recs.push(Recommendation {
                    recommendation_type: RecommendationType::NPlusOneQuery,
                    table: None,
                    columns: vec![],
                    description: "Correlated subquery detected — executes once per row of the outer query, causing N+1 performance".to_string(),
                    estimated_improvement: 0.6,
                    sql_suggestion: Some("Rewrite as a JOIN, LATERAL join, or use EXISTS/IN depending on cardinality".to_string()),
                    confidence: ConfidenceTier::SyntacticGuess,
                });
            }
        }
        Expr::BinaryOp { left, right, .. } => {
            check_correlated_subqueries(left, recs);
            check_correlated_subqueries(right, recs);
        }
        Expr::Nested(inner) => {
            check_correlated_subqueries(inner, recs);
        }
        _ => {}
    }
}

/// Check if a SELECT query references a table that has foreign keys to other tables
/// but doesn't JOIN those tables — suggesting missing eager loading / JOIN.
fn check_missing_joins(
    select: &sqlparser::ast::Select,
    schema: &SchemaSnapshot,
    recs: &mut Vec<Recommendation>,
) {
    // Get the main table name
    let main_table = match &select.from.first() {
        Some(from) => match &from.relation {
            TableFactor::Table { name, .. } => name.0.last().map(|n| n.value.as_str()),
            _ => None,
        },
        None => return,
    };

    let main_table = match main_table {
        Some(t) => t,
        None => return,
    };

    // Find the table schema
    let table_schema = match schema.tables.iter().find(|t| t.name == main_table) {
        Some(ts) => ts,
        None => return,
    };

    // Check if the SELECT projection references columns from FK-related tables
    // but those tables aren't in the FROM/JOIN list
    let joined_tables: Vec<String> = select
        .from
        .iter()
        .flat_map(|from| {
            std::iter::once(match &from.relation {
                TableFactor::Table { name, .. } => {
                    name.0.last().map(|n| n.value.clone()).unwrap_or_default()
                }
                _ => String::new(),
            })
            .chain(from.joins.iter().map(|join| match &join.relation {
                TableFactor::Table { name, .. } => {
                    name.0.last().map(|n| n.value.clone()).unwrap_or_default()
                }
                _ => String::new(),
            }))
        })
        .collect();

    // For each FK from this table, check if the referenced table appears in projections
    // but not in FROM/JOIN — classic N+1 eager-loading miss
    for fk in &table_schema.foreign_keys {
        let ref_table = &fk.referenced_table;
        if !joined_tables.iter().any(|t| t == ref_table) {
            // Check if any column in the SELECT list might come from the referenced table
            let select_text = select
                .projection
                .iter()
                .map(|p| p.to_string())
                .collect::<String>()
                .to_lowercase();
            let ref_schema = schema.tables.iter().find(|t| t.name == *ref_table);
            if let Some(ref_ts) = ref_schema {
                let has_ref_column = ref_ts
                    .columns
                    .iter()
                    .any(|col| select_text.contains(&col.name.to_lowercase()));
                if has_ref_column {
                    recs.push(Recommendation {
                        recommendation_type: RecommendationType::NPlusOneQuery,
                        table: Some(main_table.to_string()),
                        columns: fk.columns.clone(),
                        description: format!(
                            "Table '{}' has FK '{}' to '{}' — query selects columns from '{}' without JOIN, likely causing N+1 queries in application code",
                            main_table, fk.name, ref_table, ref_table
                        ),
                        estimated_improvement: 0.7,
                        sql_suggestion: Some(format!(
                            "Add JOIN {} ON {}.{} = {}.{} to eagerly load related data",
                            ref_table,
                            main_table,
                            fk.columns.first().unwrap_or(&String::new()),
                            ref_table,
                            fk.referenced_columns.first().unwrap_or(&String::new()),
                        )),
                        confidence: ConfidenceTier::SchemaVerified,
                    });
                }
            }
        }
    }
}
