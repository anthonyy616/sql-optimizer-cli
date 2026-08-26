use crate::core::types::{AnalysisResult, Profile, Recommendation};

/// Apply profile-aware policy to an analysis result:
/// - OLTP: latency-focused. Drops analytics-only recommendations (dollar estimates,
///   partitioning advice) and surfaces index/N+1/join issues first.
/// - Analytics: throughput/cost-focused. Boosts partitioning and cost recommendations;
///   demotes single-row OLTP concerns (N+1) below structural ones.
pub fn apply_profile_policy(result: &mut AnalysisResult) {
    let mut recs = std::mem::take(&mut result.recommendations);
    recs = dedup_recommendations(recs);

    match result.profile {
        Profile::Oltp => {
            recs.retain(|rec| !is_analytics_only(rec));
            recs.sort_by(|a, b| {
                oltp_weight(b)
                    .total_cmp(&oltp_weight(a))
                    .then(b.estimated_improvement.total_cmp(&a.estimated_improvement))
            });
        }
        Profile::Analytics => {
            recs.sort_by(|a, b| {
                analytics_weight(b)
                    .total_cmp(&analytics_weight(a))
                    .then(b.estimated_improvement.total_cmp(&a.estimated_improvement))
            });
        }
    }

    result.recommendations = recs;
}

fn is_analytics_only(rec: &Recommendation) -> bool {
    let d = rec.description.to_lowercase();
    d.contains("estimated compute cost") || d.contains("partitioning")
}

fn is_cost_related(rec: &Recommendation) -> bool {
    let d = rec.description.to_lowercase();
    d.contains("cost") || d.contains("partition") || d.contains("scan")
}

/// Higher weight = shown first for the OLTP audience.
fn oltp_weight(rec: &Recommendation) -> f64 {
    let base = match rec.recommendation_type {
        crate::core::types::RecommendationType::MissingIndex => 5.0,
        crate::core::types::RecommendationType::NPlusOneQuery => 4.0,
        crate::core::types::RecommendationType::InefficientJoin => 3.5,
        crate::core::types::RecommendationType::CartesianProduct => 4.5,
        crate::core::types::RecommendationType::QueryRewrite => 2.0,
    };
    // Schema/plan-verified findings matter more than guesses in OLTP debugging.
    let confidence_boost = match rec.confidence {
        crate::core::types::ConfidenceTier::PlanVerified => 2.0,
        crate::core::types::ConfidenceTier::SchemaVerified => 1.5,
        crate::core::types::ConfidenceTier::SyntacticGuess => 0.0,
        crate::core::types::ConfidenceTier::OrmHeuristic => 0.5,
    };
    base + confidence_boost
}

/// Higher weight = shown first for the analytics audience.
fn analytics_weight(rec: &Recommendation) -> f64 {
    let mut weight = rec.estimated_improvement * 10.0;
    if is_cost_related(rec) {
        weight += 4.0;
    }
    // Single-row access patterns are mostly irrelevant to batch analytics jobs.
    if matches!(
        rec.recommendation_type,
        crate::core::types::RecommendationType::NPlusOneQuery
    ) {
        weight -= 3.0;
    }
    weight
}

/// Remove duplicate recommendations that carry identical descriptions,
/// keeping the highest-improvement variant.
fn dedup_recommendations(recs: Vec<Recommendation>) -> Vec<Recommendation> {
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut out: Vec<Recommendation> = Vec::new();
    for rec in recs {
        if let Some(&idx) = seen.get(&rec.description) {
            if rec.estimated_improvement > out[idx].estimated_improvement {
                out[idx] = rec;
            }
        } else {
            seen.insert(rec.description.clone(), out.len());
            out.push(rec);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::*;

    fn rec(desc: &str, rtype: RecommendationType, improvement: f64) -> Recommendation {
        Recommendation {
            recommendation_type: rtype,
            table: None,
            columns: vec![],
            description: desc.to_string(),
            estimated_improvement: improvement,
            sql_suggestion: None,
            confidence: ConfidenceTier::SchemaVerified,
        }
    }

    fn result(profile: Profile, recs: Vec<Recommendation>) -> AnalysisResult {
        AnalysisResult {
            query: "SELECT 1".into(),
            database_type: DatabaseType::PostgreSQL,
            profile,
            recommendations: recs,
            security_score: 100.0,
            security_issues: vec![],
            schema_snapshot: None,
            explain_plan: None,
            row_preview: None,
            execution_time_ms: 1,
            regressions: vec![],
            schema_drift: vec![],
        }
    }

    #[test]
    fn oltp_profile_drops_dollar_estimates_and_ranks_indexes_first() {
        let mut result = result(
            Profile::Oltp,
            vec![
                rec(
                    "Estimated compute cost: $0.5 — approximation",
                    RecommendationType::QueryRewrite,
                    0.9,
                ),
                rec(
                    "Missing index on users(email)",
                    RecommendationType::MissingIndex,
                    0.7,
                ),
                rec(
                    "IN subquery detected",
                    RecommendationType::NPlusOneQuery,
                    0.6,
                ),
            ],
        );
        apply_profile_policy(&mut result);
        assert_eq!(result.recommendations.len(), 2);
        assert!(matches!(
            result.recommendations[0].recommendation_type,
            RecommendationType::MissingIndex
        ));
    }

    #[test]
    fn analytics_profile_keeps_cost_recs_and_boosts_them() {
        let mut result = result(
            Profile::Analytics,
            vec![
                rec(
                    "IN subquery detected",
                    RecommendationType::NPlusOneQuery,
                    0.6,
                ),
                rec(
                    "Consider declarative partitioning on timestamp column",
                    RecommendationType::QueryRewrite,
                    0.4,
                ),
                rec(
                    "Missing index on events(created_at)",
                    RecommendationType::MissingIndex,
                    0.8,
                ),
            ],
        );
        apply_profile_policy(&mut result);
        assert_eq!(result.recommendations.len(), 3);
        // Cost-related (partitioning + missing index on scan column) should outrank N+1.
        assert!(!matches!(
            result.recommendations[0].recommendation_type,
            RecommendationType::NPlusOneQuery
        ));
    }
}
