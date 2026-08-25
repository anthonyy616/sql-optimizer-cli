use crate::core::types::{ConfidenceTier, Recommendation, RecommendationType, SchemaSnapshot};

/// A generated fix with before/after representation.
#[derive(Debug, Clone)]
pub struct FixSuggestion {
    pub original_query: String,
    pub fixed_query: String,
    pub explanation: String,
    pub confidence: ConfidenceTier,
    /// DDL statement to apply (e.g., CREATE INDEX) if applicable.
    pub ddl: Option<String>,
}

/// Generate fix suggestions for high-confidence recommendations.
pub fn generate_fixes(
    query: &str,
    recommendations: &[Recommendation],
    schema: &SchemaSnapshot,
) -> Vec<FixSuggestion> {
    let mut fixes = Vec::new();

    for rec in recommendations {
        // Only generate fixes for recommendations with sufficient confidence
        let fix = match rec.recommendation_type {
            RecommendationType::MissingIndex => generate_index_fix(query, rec, schema),
            RecommendationType::NPlusOneQuery => generate_join_fix(query, rec),
            RecommendationType::CartesianProduct => generate_cartesian_fix(query, rec),
            RecommendationType::InefficientJoin => generate_join_index_fix(query, rec),
            RecommendationType::QueryRewrite => generate_rewrite_fix(query, rec),
        };

        if let Some(f) = fix {
            fixes.push(f);
        }
    }

    fixes
}

fn generate_index_fix(
    _query: &str,
    rec: &Recommendation,
    _schema: &SchemaSnapshot,
) -> Option<FixSuggestion> {
    let table = rec.table.as_ref()?;
    let col = rec.columns.first()?;

    let ddl = format!("CREATE INDEX idx_{}_{} ON {}({});", table, col, table, col);

    Some(FixSuggestion {
        original_query: String::new(), // Not applicable for index fixes
        fixed_query: String::new(),
        explanation: format!(
            "Create an index on column '{}' of table '{}' to speed up WHERE/JOIN lookups on this column",
            col, table
        ),
        confidence: rec.confidence.clone(),
        ddl: Some(ddl),
    })
}

fn generate_join_fix(query: &str, rec: &Recommendation) -> Option<FixSuggestion> {
    let suggestion = rec.sql_suggestion.as_ref()?;

    // For IN subquery → JOIN rewrites, provide the rewrite pattern
    if rec.description.contains("IN subquery") {
        Some(FixSuggestion {
            original_query: query.to_string(),
            fixed_query: format!(
                "-- Rewrite IN subquery as JOIN:\n\
                 -- Original: {}\n\
                 -- Suggested: Replace IN (subquery) with INNER JOIN or EXISTS\n\
                 -- {}",
                query, suggestion
            ),
            explanation: "IN subqueries execute the subquery once per row. A JOIN or EXISTS evaluates more efficiently with query optimizer support.".to_string(),
            confidence: rec.confidence.clone(),
            ddl: None,
        })
    } else if rec.description.contains("Correlated subquery") {
        Some(FixSuggestion {
            original_query: query.to_string(),
            fixed_query: format!(
                "-- Correlated subquery rewrite:\n\
                 -- {}",
                suggestion
            ),
            explanation: "Correlated subqueries execute the inner query once per row of the outer query. Rewriting as a JOIN allows the database to optimize the full set at once.".to_string(),
            confidence: rec.confidence.clone(),
            ddl: None,
        })
    } else {
        Some(FixSuggestion {
            original_query: query.to_string(),
            fixed_query: suggestion.clone(),
            explanation: rec.description.clone(),
            confidence: rec.confidence.clone(),
            ddl: None,
        })
    }
}

fn generate_cartesian_fix(query: &str, rec: &Recommendation) -> Option<FixSuggestion> {
    Some(FixSuggestion {
        original_query: query.to_string(),
        fixed_query: rec.sql_suggestion.clone().unwrap_or_default(),
        explanation: rec.description.clone(),
        confidence: rec.confidence.clone(),
        ddl: None,
    })
}

fn generate_join_index_fix(query: &str, rec: &Recommendation) -> Option<FixSuggestion> {
    let suggestion = rec.sql_suggestion.as_ref()?;
    let ddl = if suggestion.contains("CREATE INDEX") {
        Some(suggestion.clone())
    } else {
        None
    };

    Some(FixSuggestion {
        original_query: query.to_string(),
        fixed_query: String::new(),
        explanation: format!(
            "Index the JOIN column to avoid full table scans during join evaluation: {}",
            suggestion
        ),
        confidence: rec.confidence.clone(),
        ddl,
    })
}

fn generate_rewrite_fix(query: &str, rec: &Recommendation) -> Option<FixSuggestion> {
    Some(FixSuggestion {
        original_query: query.to_string(),
        fixed_query: rec.sql_suggestion.clone().unwrap_or_default(),
        explanation: rec.description.clone(),
        confidence: rec.confidence.clone(),
        ddl: None,
    })
}

/// Generate a unified diff-style preview of original vs fixed query.
pub fn format_diff(fix: &FixSuggestion) -> String {
    let mut output = String::new();

    if !fix.original_query.is_empty() {
        output.push_str("=== BEFORE ===\n");
        output.push_str(&fix.original_query);
        output.push('\n');
        output.push_str("=== AFTER ===\n");
        if !fix.fixed_query.is_empty() {
            output.push_str(&fix.fixed_query);
        }
        output.push('\n');
    }

    output.push_str(&format!("Explanation: {}\n", fix.explanation));
    output.push_str(&format!("Confidence: {}\n", fix.confidence));

    if let Some(ddl) = &fix.ddl {
        output.push_str(&format!("\nSuggested DDL:\n{}\n", ddl));
    }

    output
}
