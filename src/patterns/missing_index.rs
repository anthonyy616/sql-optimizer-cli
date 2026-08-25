use crate::core::types::{ConfidenceTier, Recommendation, RecommendationType, SchemaSnapshot};
use regex::Regex;

pub fn detect_missing_index(query: &str, schema: &SchemaSnapshot) -> Vec<Recommendation> {
    let mut recs = Vec::new();

    // Heuristic: find the main table from FROM clause
    let from_re = Regex::new(r"(?i)from\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap();
    let where_re =
        Regex::new(r"(?i)where\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*(=|like|ilike|in\b)").unwrap();

    let table = from_re
        .captures(query)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string());

    let mut where_cols = Vec::new();
    for caps in where_re.captures_iter(query) {
        if let Some(m) = caps.get(1) {
            where_cols.push(m.as_str().to_string());
        }
    }

    if where_cols.is_empty() || table.is_none() {
        return recs;
    }

    let table_name = table.unwrap();
    // Find table schema
    if let Some(ts) = schema.tables.iter().find(|t| t.name == table_name) {
        for col in where_cols {
            let indexed = ts
                .indexes
                .iter()
                .any(|idx| idx.columns.iter().any(|c| c == &col));
            if !indexed {
                recs.push(Recommendation {
                    recommendation_type: RecommendationType::MissingIndex,
                    table: Some(table_name.clone()),
                    columns: vec![col.clone()],
                    description: format!(
                        "Missing index on column '{}' of table '{}'",
                        col, table_name
                    ),
                    estimated_improvement: 0.5,
                    sql_suggestion: Some(format!(
                        "CREATE INDEX idx_{}_{} ON {}({});",
                        table_name, col, table_name, col
                    )),
                    confidence: ConfidenceTier::SchemaVerified,
                });
            }
        }
    }

    recs
}
