use crate::core::types::{ConfidenceTier, Recommendation, RecommendationType};

/// An ORM/framework recognized from query shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrmSignature {
    ActiveRecord,
    Django,
    Prisma,
    Knex,
}

impl OrmSignature {
    pub fn name(&self) -> &'static str {
        match self {
            OrmSignature::ActiveRecord => "Rails/ActiveRecord",
            OrmSignature::Django => "Django ORM",
            OrmSignature::Prisma => "Prisma",
            OrmSignature::Knex => "Knex",
        }
    }
}

/// Result of shape-only ORM analysis for one query.
pub struct OrmAnalysis {
    pub signature: Option<OrmSignature>,
    pub recommendations: Vec<Recommendation>,
}

/// Analyze a single query for ORM-generated shapes and known ORM anti-patterns.
///
/// This is explicitly heuristic (design decision 16): findings are always
/// labeled `orm-heuristic` and are never elevated to schema/plan-verified on
/// their own. Shape-only mode works without any source context; when project
/// scanning supplies an origin file, callers can mention it in messaging but
/// the confidence tier does not change.
pub fn detect_orm_patterns(query: &str) -> OrmAnalysis {
    let normalized = collapse_whitespace(query);
    let mut signature: Option<OrmSignature> = None;

    // --- Signature detection (first match wins; most specific first) ---

    // Prisma: schema-qualified CamelCase tables with positional aliases t0/t1/t2...
    if normalized.contains("t0.")
        && (normalized.contains("\"public\".") || re_looks_like_prisma(&normalized))
    {
        signature = Some(OrmSignature::Prisma);
    }
    // ActiveRecord: table-quoted star `"users".*` or `ORDER BY ... LIMIT ?` with
    // fully backtick/quote-qualified identifiers typical of Rails-generated SQL.
    if signature.is_none()
        && (normalized.contains(".* from") || re_looks_like_activerecord(&normalized))
    {
        signature = Some(OrmSignature::ActiveRecord);
    }
    // Django: explicit fully-qualified column lists in double quotes
    // `SELECT "app_user"."id", "app_user"."name" FROM "app_user" WHERE ...`
    if signature.is_none() && re_looks_like_django(&normalized) {
        signature = Some(OrmSignature::Django);
    }
    // Knex: backtick-quoted aliasing (`x`.`y`) with `. as ``alias```
    if signature.is_none() && normalized.contains(" as `") {
        signature = Some(OrmSignature::Knex);
    }

    let mut recommendations = Vec::new();

    // --- Known anti-pattern shapes ---
    if let Some(orm) = signature {
        // Over-fetch: ORM star-select or all-column select pulls every column.
        let over_fetches = normalized.contains(".* ")
            || normalized.contains("select * ")
            || count_pattern(&normalized, "t0.\"") >= 5  // Prisma positional-alias columns
            || count_pattern(&normalized, \", \"") >= 5; // quoted column lists
        if over_fetches {
            recommendations.push(Recommendation {
                recommendation_type: RecommendationType::QueryRewrite,
                table: None,
                columns: vec![],
                description: format!(
                    "Query looks like {} output and selects all columns — consider a projection or \
                     (for Prisma) a `select:` clause to avoid over-fetching wide rows",
                    orm.name()
                ),
                estimated_improvement: 0.3,
                sql_suggestion: None,
                confidence: ConfidenceTier::OrmHeuristic,
            });
        }

        // Missing eager loading: child-table lookup filtered by a foreign-key-style
        // equality is the classic N+1 access pattern when issued inside a loop by
        // an ORM that did not preload associations.
        if looks_like_child_lookup(&normalized) {
            recommendations.push(Recommendation {
                recommendation_type: RecommendationType::NPlusOneQuery,
                table: None,
                columns: vec![],
                description: format!(
                    "Query looks like {} output fetching child rows by association key — if this \
                     runs per parent record you have an N+1; use eager loading \
                     (includes/preload/include/select-related or Prisma `include:`)",
                    orm.name()
                ),
                estimated_improvement: 0.6,
                sql_suggestion: None,
                confidence: ConfidenceTier::OrmHeuristic,
            });
        }
    }

    OrmAnalysis {
        signature,
        recommendations,
    }
}

/// Human-readable detection note for verbose output / scan reports.
pub fn describe_signature(sig: OrmSignature) -> String {
    format!("query shape matches {}", sig.name())
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for ch in s.chars() {
        let is_space = ch.is_whitespace();
        if is_space {
            if !prev_space {
                out.push(' ');
            }
        } else {
            out.push(ch);
        }
        prev_space = is_space;
    }
    out.to_lowercase()
}

/// `"users".* FROM "users"` — Rails generates exactly this for `User.select(:all)` /
/// default finder output on Postgres.
fn re_looks_like_activerecord(normalized: &str) -> bool {
    normalized.contains("\".* from \"") || normalized.contains("`.* from `")
}

/// Django always emits fully-quoted explicit column lists:
/// `select "a"."b", "a"."c" from "a"` — at least two quoted qualified columns.
fn re_looks_like_django(normalized: &str) -> bool {
    count_pattern(normalized, "\".\"") >= 2 && normalized.contains("\" from \"")
}

/// Prisma uses schema-qualified tables plus positional aliases:
/// `from "public"."user" t0 where t0."id" = $1`
fn re_looks_like_prisma(normalized: &str) -> bool {
    (normalized.contains("\"public\".") || normalized.contains("\"main\"."))
        && (normalized.contains(" t0 ") || normalized.contains("(t0") || normalized.contains("t0."))
}

/// Count foreign-key-shaped equality filters: `<alias>.<something>_id = <param>`
/// or `... WHERE <table>`.`<parent>_id` = ...
fn looks_like_child_lookup(normalized: &str) -> bool {
    let has_fk_filter = (normalized.contains("_id = $") || normalized.contains("_id\" = $"))
        || (normalized.contains("_id = ?") || normalized.contains("_id\" = ?"))
        || (normalized.contains("_id = %") || normalized.contains("_id\" = %"));
    has_fk_filter && !normalized.contains(" id = ")
}

fn count_quoted_columns(s: &str) -> usize {
    count_pattern(s, "\", \"")
}

fn count_pattern(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    haystack.matches(needle).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_active_record_star_select() {
        let q = r#"SELECT "users".* FROM "users" WHERE "users"."id" = $1 ORDER BY "users"."id" ASC LIMIT 1"#;
        let analysis = detect_orm_patterns(q);
        assert_eq!(analysis.signature, Some(OrmSignature::ActiveRecord));
        assert!(analysis
            .recommendations
            .iter()
            .any(|r| r.confidence == ConfidenceTier::OrmHeuristic));
    }

    #[test]
    fn detects_prisma_over_fetch() {
        let q = r#"SELECT t0."id", t0."name", t0."email", t0."created_at", t0."updated_at", t0."deleted_at" FROM "public"."User" t0 WHERE t0."id" = $1 LIMIT 1"#;
        let analysis = detect_orm_patterns(q);
        assert_eq!(analysis.signature, Some(OrmSignature::Prisma));
        assert!(analysis
            .recommendations
            .iter()
            .any(|r| r.description.contains("over-fetch")));
    }

    #[test]
    fn detects_orm_n_plus_one_shape() {
        let q = r#"SELECT "orders".* FROM "orders" WHERE "orders"."user_id" = $1"#;
        let analysis = detect_orm_patterns(q);
        assert_eq!(analysis.signature, Some(OrmSignature::ActiveRecord));
        assert!(analysis
            .recommendations
            .iter()
            .any(|r| matches!(r.recommendation_type, RecommendationType::NPlusOneQuery)));
    }

    #[test]
    fn handwritten_query_is_not_misclassified() {
        let q = "SELECT u.email, COUNT(o.id) FROM users u JOIN orders o ON o.user_id = u.id GROUP BY u.email";
        let analysis = detect_orm_patterns(q);
        assert_eq!(analysis.signature, None);
        assert!(analysis.recommendations.is_empty());
    }
}
