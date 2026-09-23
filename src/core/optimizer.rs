//! Recommendation aggregation for the Optimize workspace.
//!
//! Scope guard: this module only aggregates, deduplicates, and ranks findings
//! that the analyzer, schema checks, and rewriter already produced. Automatic
//! mutation, benchmarking, and write execution are explicitly out of scope —
//! the TUI presents generated SQL/diffs as copy/preview only.

use crate::core::types::{AnalysisResult, ConfidenceTier, Recommendation, RecommendationType};

/// A grouped, ranked recommendation with provenance metadata for the Optimize tab.
#[derive(Debug, Clone)]
pub struct OptimizeFinding {
    pub recommendation: Recommendation,
    /// Confidence label including whether the finding was verified against the
    /// live schema/plan context (`true`) or is heuristic-only (`false`).
    pub verified: bool,
    /// Where the finding came from (e.g. "schema check", "query shape", "rewriter").
    pub source: &'static str,
}

impl OptimizeFinding {
    /// Rank weight: higher floats first. Type priority x confidence x impact.
    pub fn rank(&self) -> f64 {
        let type_weight = match self.recommendation.recommendation_type {
            RecommendationType::MissingIndex => 5.0,
            RecommendationType::CartesianProduct => 4.5,
            RecommendationType::NPlusOneQuery => 4.0,
            RecommendationType::InefficientJoin => 3.5,
            RecommendationType::QueryRewrite => 2.0,
        };
        let confidence_weight = match self.recommendation.confidence {
            ConfidenceTier::PlanVerified => 2.0,
            ConfidenceTier::SchemaVerified => 1.5,
            ConfidenceTier::OrmHeuristic => 0.5,
            ConfidenceTier::SyntacticGuess => 0.0,
        };
        let verification_bonus = if self.verified { 1.0 } else { 0.0 };
        type_weight
            + confidence_weight
            + verification_bonus
            + self.recommendation.estimated_improvement
    }
}

/// Aggregate an analysis result into Optimize-tab findings, grouped by
/// category: missing indexes, inefficient joins, cartesian products, N+1
/// patterns, and query rewrites.
pub fn collect_findings(result: &AnalysisResult) -> Vec<OptimizeFinding> {
    let schema_attached = result.schema_snapshot.is_some();
    let plan_attached = result.explain_plan.is_some();

    result
        .recommendations
        .iter()
        // "Fix:" entries duplicate DDL from the rewriter into other recs; the
        // rewriter output is surfaced separately via generate_fixes.
        .filter(|rec| !rec.description.starts_with("Fix: "))
        .map(|rec| {
            // A finding is only "verified" when its confidence tier proves it:
            // schema-verified tiers require an attached schema, plan-verified
            // requires an attached plan.
            let verified = match rec.confidence {
                ConfidenceTier::SchemaVerified => schema_attached,
                ConfidenceTier::PlanVerified => plan_attached,
                ConfidenceTier::OrmHeuristic | ConfidenceTier::SyntacticGuess => false,
            };
            let source = match rec.confidence {
                ConfidenceTier::PlanVerified => "EXPLAIN plan",
                ConfidenceTier::SchemaVerified => "schema check",
                ConfidenceTier::OrmHeuristic => "ORM pattern",
                ConfidenceTier::SyntacticGuess => "query shape",
            };
            OptimizeFinding {
                recommendation: rec.clone(),
                verified,
                source,
            }
        })
        .collect()
}

/// Sort findings by rank (best first) in place.
pub fn rank_findings(findings: &mut [OptimizeFinding]) {
    findings.sort_by(|a, b| {
        b.rank().total_cmp(&a.rank()).then_with(|| {
            a.recommendation
                .description
                .cmp(&b.recommendation.description)
        })
    });
}

/// Convenience: collect + rank in one call.
pub fn ranked_findings(result: &AnalysisResult) -> Vec<OptimizeFinding> {
    let mut findings = collect_findings(result);
    rank_findings(&mut findings);
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{DatabaseType, Profile, SchemaSnapshot};

    fn rec(
        rtype: RecommendationType,
        confidence: ConfidenceTier,
        improvement: f64,
        description: &str,
    ) -> Recommendation {
        Recommendation {
            recommendation_type: rtype,
            table: Some("users".into()),
            columns: vec!["email".into()],
            description: description.into(),
            estimated_improvement: improvement,
            sql_suggestion: Some("SELECT 1".into()),
            confidence,
        }
    }

    fn result(recommendations: Vec<Recommendation>, schema: bool, plan: bool) -> AnalysisResult {
        AnalysisResult {
            query: "SELECT * FROM users".into(),
            database_type: DatabaseType::SQLite,
            profile: Profile::Oltp,
            recommendations,
            security_score: 100.0,
            security_issues: vec![],
            schema_snapshot: schema.then(SchemaSnapshot::default),
            explain_plan: plan.then_some(Default::default()),
            row_preview: None,
            execution_time_ms: 1,
            regressions: vec![],
            schema_drift: vec![],
        }
    }

    #[test]
    fn heuristic_findings_are_not_verified_without_context() {
        let r = result(
            vec![rec(
                RecommendationType::MissingIndex,
                ConfidenceTier::SyntacticGuess,
                0.5,
                "missing index",
            )],
            false,
            false,
        );
        let findings = ranked_findings(&r);
        assert_eq!(findings.len(), 1);
        assert!(!findings[0].verified);
        assert_eq!(findings[0].source, "query shape");
    }

    #[test]
    fn schema_verified_requires_attached_schema() {
        let with_schema = result(
            vec![rec(
                RecommendationType::MissingIndex,
                ConfidenceTier::SchemaVerified,
                0.5,
                "missing index",
            )],
            true,
            false,
        );
        assert!(ranked_findings(&with_schema)[0].verified);

        let without_schema = result(
            vec![rec(
                RecommendationType::MissingIndex,
                ConfidenceTier::SchemaVerified,
                0.5,
                "missing index",
            )],
            false,
            false,
        );
        assert!(!ranked_findings(&without_schema)[0].verified);
    }

    #[test]
    fn plan_verified_requires_attached_plan() {
        let with_plan = result(
            vec![rec(
                RecommendationType::MissingIndex,
                ConfidenceTier::PlanVerified,
                0.5,
                "missing index",
            )],
            true,
            true,
        );
        assert!(ranked_findings(&with_plan)[0].verified);

        let without_plan = result(
            vec![rec(
                RecommendationType::MissingIndex,
                ConfidenceTier::PlanVerified,
                0.5,
                "missing index",
            )],
            true,
            false,
        );
        assert!(!ranked_findings(&without_plan)[0].verified);
    }

    #[test]
    fn verified_findings_rank_above_equal_heuristics() {
        let r = result(
            vec![
                rec(
                    RecommendationType::QueryRewrite,
                    ConfidenceTier::SyntacticGuess,
                    0.9,
                    "heuristic rewrite",
                ),
                rec(
                    RecommendationType::MissingIndex,
                    ConfidenceTier::SchemaVerified,
                    0.4,
                    "verified index",
                ),
            ],
            true,
            false,
        );
        let findings = ranked_findings(&r);
        assert_eq!(findings[0].recommendation.description, "verified index");
    }

    #[test]
    fn rewriter_fix_duplicates_are_excluded() {
        let r = result(
            vec![rec(
                RecommendationType::QueryRewrite,
                ConfidenceTier::SchemaVerified,
                0.5,
                "Fix: Create an index on column 'email'",
            )],
            true,
            false,
        );
        assert!(ranked_findings(&r).is_empty());
    }
}
