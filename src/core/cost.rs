use crate::core::types::{AnalysisResult, Profile, Recommendation, RecommendationType};
use sqlparser::ast::{SetExpr, Statement};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;

/// Cost estimation output for analytics-aware recommendations.
#[derive(Debug, Clone)]
pub struct CostEstimate {
    /// Estimated number of rows scanned
    pub rows_scanned: Option<f64>,
    /// Estimated bytes scanned
    pub bytes_scanned: Option<f64>,
    /// Number of tables involved
    pub table_count: usize,
    /// Number of JOINs
    pub join_count: usize,
    /// Whether the query uses aggregation
    pub has_aggregation: bool,
    /// Whether the query uses sorting
    pub has_sort: bool,
    /// Whether partition pruning could help
    pub partitioning_candidate: bool,
    /// Rough dollar estimate (labeled as estimate)
    pub dollar_estimate: Option<f64>,
}

/// Analyze a query's cost characteristics based on its AST structure and plan.
pub fn estimate_query_cost(result: &AnalysisResult) -> CostEstimate {
    let dialect = GenericDialect {};
    let statements = Parser::parse_sql(&dialect, &result.query).ok();

    let mut table_count = 0usize;
    let mut join_count = 0usize;
    let mut has_aggregation = false;
    let mut has_sort = false;

    if let Some(stmts) = &statements {
        for stmt in stmts {
            if let Statement::Query(query_box) = stmt {
                if let SetExpr::Select(select) = &*query_box.body {
                    // Count tables in FROM
                    for from in &select.from {
                        table_count += 1; // main table
                        join_count += from.joins.len();
                    }

                    // Check for GROUP BY, HAVING, aggregate functions
                    let has_group_by = match &select.group_by {
                        sqlparser::ast::GroupByExpr::Expressions(exprs) => !exprs.is_empty(),
                        _ => false,
                    };
                    if has_group_by || select.having.is_some() {
                        has_aggregation = true;
                    }
                    let select_text = select
                        .projection
                        .iter()
                        .map(|p| p.to_string())
                        .collect::<String>()
                        .to_lowercase();
                    if select_text.contains("count(")
                        || select_text.contains("sum(")
                        || select_text.contains("avg(")
                        || select_text.contains("max(")
                        || select_text.contains("min(")
                    {
                        has_aggregation = true;
                    }

                    // Check for ORDER BY
                    if !query_box.order_by.is_empty() {
                        has_sort = true;
                    }
                }
            }
        }
    }

    // Extract rows/cost from EXPLAIN plan if available
    let (rows_scanned, bytes_scanned) = if let Some(plan) = &result.explain_plan {
        extract_plan_cost(plan)
    } else {
        (None, None)
    };

    // Partitioning candidate: large table filtered on a single column (e.g. timestamp)
    let partitioning_candidate = table_count >= 1
        && result.query.to_lowercase().contains("where")
        && (result.query.to_lowercase().contains("date")
            || result.query.to_lowercase().contains("timestamp")
            || result.query.to_lowercase().contains("created_at")
            || result.query.to_lowercase().contains("updated_at")
            || result.query.to_lowercase().contains("time"));

    // Rough dollar estimate based on rows and joins
    // This is a very rough heuristic: ~$0.0001 per 1000 rows scanned + $0.001 per join
    let dollar_estimate = rows_scanned.map(|rows| {
        let row_cost = (rows / 1000.0) * 0.0001;
        let join_cost = join_count as f64 * 0.001;
        let sort_cost = if has_sort { 0.005 } else { 0.0 };
        let agg_cost = if has_aggregation { 0.002 } else { 0.0 };
        let raw = row_cost + join_cost + sort_cost + agg_cost;
        (raw * 10000.0).round() / 10000.0
    });

    CostEstimate {
        rows_scanned,
        bytes_scanned,
        table_count,
        join_count,
        has_aggregation,
        has_sort,
        partitioning_candidate,
        dollar_estimate,
    }
}

fn extract_plan_cost(plan: &crate::core::types::QueryPlan) -> (Option<f64>, Option<f64>) {
    let root = match &plan.root {
        Some(r) => r,
        None => return (None, None),
    };

    // Sum rows and cost across all nodes
    let mut total_rows = 0.0;
    let mut total_cost = 0.0;
    collect_plan_stats(root, &mut total_rows, &mut total_cost);

    let rows = if total_rows > 0.0 {
        Some(total_rows)
    } else {
        None
    };
    let cost = if total_cost > 0.0 {
        Some(total_cost)
    } else {
        None
    };
    (rows, cost)
}

fn collect_plan_stats(node: &crate::core::types::QueryPlanNode, rows: &mut f64, cost: &mut f64) {
    if let Some(r) = node.rows {
        *rows += r;
    }
    if let Some(c) = node.cost {
        *cost += c;
    }
    for child in &node.children {
        collect_plan_stats(child, rows, cost);
    }
}

/// Generate analytics-specific recommendations based on cost estimates.
pub fn generate_analytics_recommendations(
    estimate: &CostEstimate,
    profile: &Profile,
) -> Vec<Recommendation> {
    let mut recs = Vec::new();

    // Only generate analytics-specific recommendations when in analytics profile
    if *profile != Profile::Analytics {
        return recs;
    }

    // High scan volume without partitioning
    if let Some(rows) = estimate.rows_scanned {
        if rows > 100_000.0 && estimate.partitioning_candidate {
            recs.push(Recommendation {
                recommendation_type: RecommendationType::QueryRewrite,
                table: None,
                columns: vec![],
                description: format!(
                    "Query scans ~{} rows on a time-filtered table — consider declarative partitioning (e.g. Postgres RANGE partitioning on the timestamp column)",
                    format_number(rows as i64)
                ),
                estimated_improvement: 0.4,
                sql_suggestion: Some(
                    "CREATE TABLE ... PARTITION BY RANGE (column_name);".to_string()
                ),
                confidence: crate::core::types::ConfidenceTier::SyntacticGuess,
            });
        }

        // Dollar estimate for analytics queries
        if let Some(dollars) = estimate.dollar_estimate {
            if dollars > 0.01 {
                recs.push(Recommendation {
                    recommendation_type: RecommendationType::QueryRewrite,
                    table: None,
                    columns: vec![],
                    description: format!(
                        "Estimated compute cost: ${:.4} (rows scanned: ~{}, joins: {}, sort: {}, aggregation: {}) — this is an approximation, not a bill",
                        dollars,
                        format_number(estimate.rows_scanned.unwrap_or(0.0) as i64),
                        estimate.join_count,
                        if estimate.has_sort { "yes" } else { "no" },
                        if estimate.has_aggregation { "yes" } else { "no" },
                    ),
                    estimated_improvement: 0.3,
                    sql_suggestion: None,
                    confidence: crate::core::types::ConfidenceTier::SyntacticGuess,
                });
            }
        }
    }

    // Sort without index support
    if estimate.has_sort && estimate.join_count > 0 {
        recs.push(Recommendation {
            recommendation_type: RecommendationType::QueryRewrite,
            table: None,
            columns: vec![],
            description: "Query combines JOINs with ORDER BY — ensure the sort column is indexed to avoid filesort".to_string(),
            estimated_improvement: 0.2,
            sql_suggestion: Some("Add an index on the ORDER BY column(s)".to_string()),
            confidence: crate::core::types::ConfidenceTier::SyntacticGuess,
        });
    }

    recs
}

fn format_number(n: i64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}
