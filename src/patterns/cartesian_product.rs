use crate::core::types::{ConfidenceTier, Recommendation, RecommendationType, SchemaSnapshot};
use sqlparser::ast::{JoinConstraint, JoinOperator, SetExpr, Statement, TableFactor};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;

/// Detect potential cartesian products: queries that reference multiple tables
/// without a JOIN condition, or that use explicit CROSS JOINs.
pub fn detect_cartesian_product(query: &str, _schema: &SchemaSnapshot) -> Vec<Recommendation> {
    let mut recs = Vec::new();

    let dialect = GenericDialect {};
    let statements = match Parser::parse_sql(&dialect, query) {
        Ok(s) => s,
        Err(_) => return recs,
    };

    for stmt in &statements {
        if let Statement::Query(query_box) = stmt {
            if let SetExpr::Select(select) = &*query_box.body {
                // Collect all tables referenced in FROM and JOINs
                let mut table_count: usize = 0;
                let mut has_join_condition = false;
                let mut has_cross_join = false;

                // Count tables in FROM clause
                match &select.from[0].relation {
                    TableFactor::Table { .. } => table_count += 1,
                    TableFactor::Derived { .. } => table_count += 1,
                    _ => {}
                }

                // Check JOINs
                for join in &select.from[0].joins {
                    table_count += 1;
                    match &join.join_operator {
                        JoinOperator::Inner(constraint)
                        | JoinOperator::LeftOuter(constraint)
                        | JoinOperator::RightOuter(constraint)
                        | JoinOperator::FullOuter(constraint) => {
                            if matches!(constraint, JoinConstraint::On(_)) {
                                has_join_condition = true;
                            }
                        }
                        JoinOperator::CrossJoin => {
                            has_cross_join = true;
                        }
                        JoinOperator::CrossApply | JoinOperator::OuterApply => {
                            has_join_condition = true;
                        }
                        _ => {}
                    }
                }

                // Flag cross joins explicitly
                if has_cross_join {
                    recs.push(Recommendation {
                        recommendation_type: RecommendationType::CartesianProduct,
                        table: None,
                        columns: vec![],
                        description: "CROSS JOIN detected — produces cartesian product of all row combinations".to_string(),
                        estimated_improvement: 0.8,
                        sql_suggestion: Some("Add an ON condition or replace CROSS JOIN with INNER JOIN with a join condition".to_string()),
                        confidence: ConfidenceTier::SyntacticGuess,
                    });
                }

                // Flag multiple tables with no join condition at all
                if table_count > 1 && !has_join_condition && !has_cross_join {
                    recs.push(Recommendation {
                        recommendation_type: RecommendationType::CartesianProduct,
                        table: None,
                        columns: vec![],
                        description: format!(
                            "Query references {} tables but has no JOIN condition — may produce a cartesian product",
                            table_count
                        ),
                        estimated_improvement: 0.7,
                        sql_suggestion: Some("Add JOIN conditions between all related tables".to_string()),
                        confidence: ConfidenceTier::SyntacticGuess,
                    });
                }
            }
        }
    }

    recs
}
