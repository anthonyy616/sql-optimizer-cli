use anyhow::{Context, Result};
use sqlparser::ast::{Query, Select, SetExpr, Statement};
use sqlparser::dialect::{GenericDialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect};
use sqlparser::parser::Parser;

use crate::core::types::*;

pub struct SqlAnalyzer;

impl SqlAnalyzer {
    pub fn new() -> Self {
        Self
    }

    pub fn with_database(&self) -> Self {
        Self::new()
    }

    pub fn parse_query(&self, query: &str, dialect: &str) -> Result<Vec<Statement>> {
        let dialect = match dialect.to_lowercase().as_str() {
            "postgresql" | "postgres" => {
                Box::new(PostgreSqlDialect {}) as Box<dyn sqlparser::dialect::Dialect>
            }
            "mysql" => Box::new(MySqlDialect {}) as Box<dyn sqlparser::dialect::Dialect>,
            "sqlite" => Box::new(SQLiteDialect {}) as Box<dyn sqlparser::dialect::Dialect>,
            _ => Box::new(GenericDialect {}) as Box<dyn sqlparser::dialect::Dialect>,
        };

        let statements = Parser::parse_sql(&*dialect, query)
            .with_context(|| format!("Failed to parse SQL query: {}", query))?;

        Ok(statements)
    }

    pub async fn analyze_query(
        &self,
        query: &str,
        db_type: DatabaseType,
        profile: Profile,
    ) -> Result<AnalysisResult> {
        let start_time = std::time::Instant::now();

        let dialect = match db_type {
            DatabaseType::PostgreSQL => "postgresql",
            DatabaseType::MySQL => "mysql",
            DatabaseType::SQLite => "sqlite",
        };

        let statements = self.parse_query(query, dialect)?;

        let mut recommendations = Vec::new();

        for statement in &statements {
            if let Statement::Query(query_box) = statement {
                self.analyze_select_query(
                    query_box,
                    query,
                    &mut recommendations,
                    db_type,
                    profile.clone(),
                )
                .await?;
            }
        }

        // Run security analysis via the validator (replaces inline basic_security_analysis)
        // Schema-aware checks happen in run_schema_checks when schema is available
        let (security_score, security_issues) =
            crate::security::validator::validate_security(query, &SchemaSnapshot::default());

        // Phase 6: ORM/framework shape detection works standalone (no DB needed).
        let orm = crate::patterns::orm::detect_orm_patterns(query);
        recommendations.extend(orm.recommendations);

        // Phase 4: profile-aware ordering/filtering of everything collected so far.
        let mut result = AnalysisResult {
            query: query.to_string(),
            database_type: db_type,
            profile,
            recommendations,
            security_score,
            security_issues,
            schema_snapshot: None,
            explain_plan: None,
            row_preview: Default::default(),
            execution_time_ms: 0,
            regressions: vec![],
            schema_drift: vec![],
        };
        crate::core::ranking::apply_profile_policy(&mut result);

        result.execution_time_ms = start_time.elapsed().as_millis() as u64;

        Ok(result)
    }

    /// Run all schema-dependent detectors: missing index, cartesian product,
    /// inefficient joins, N+1 patterns, and schema-aware security checks.
    pub async fn run_schema_checks(&self, result: &mut AnalysisResult) -> Result<()> {
        let schema = match &result.schema_snapshot {
            Some(s) => s.clone(),
            None => return Ok(()),
        };

        // Pattern detectors
        let mut missing_index_recs =
            crate::patterns::missing_index::detect_missing_index(&result.query, &schema);
        let mut cartesian_recs =
            crate::patterns::cartesian_product::detect_cartesian_product(&result.query, &schema);
        let mut join_recs =
            crate::patterns::inefficient_join::detect_inefficient_joins(&result.query, &schema);
        let mut n_plus_one_recs =
            crate::patterns::n_plus_one::detect_n_plus_one(&result.query, &schema);

        result.recommendations.append(&mut missing_index_recs);
        result.recommendations.append(&mut cartesian_recs);
        result.recommendations.append(&mut join_recs);
        result.recommendations.append(&mut n_plus_one_recs);

        // Schema-aware security checks (re-run with real schema)
        let (_sec_score, sec_issues) =
            crate::security::validator::validate_security(&result.query, &schema);

        // Merge security issues — keep existing, add new
        for issue in sec_issues {
            let is_dup = result.security_issues.iter().any(|existing| {
                existing.issue_type == issue.issue_type && existing.location == issue.location
            });
            if !is_dup {
                result.security_issues.push(issue);
            }
        }

        // Recompute score with all issues
        result.security_score =
            crate::security::validator::compute_security_score(&result.security_issues);

        // Generate fix suggestions via the rewriter
        let fixes = crate::rewriting::rewriter::generate_fixes(
            &result.query,
            &result.recommendations,
            &schema,
        );
        if !fixes.is_empty() {
            // Attach fix suggestions as DDL-bearing recommendations for index fixes
            for fix in fixes {
                if let Some(ddl) = &fix.ddl {
                    result.recommendations.push(Recommendation {
                        recommendation_type: RecommendationType::QueryRewrite,
                        table: None,
                        columns: vec![],
                        description: format!("Fix: {}", fix.explanation),
                        estimated_improvement: 0.5,
                        sql_suggestion: Some(ddl.clone()),
                        confidence: fix.confidence.clone(),
                    });
                }
            }
        }

        // Phase 3.6: cost-aware analytics recommendations
        let cost_estimate = crate::core::cost::estimate_query_cost(result);
        let mut cost_recs =
            crate::core::cost::generate_analytics_recommendations(&cost_estimate, &result.profile);
        result.recommendations.append(&mut cost_recs);

        // Phase 4: re-apply profile policy now that all detectors have contributed.
        crate::core::ranking::apply_profile_policy(result);

        Ok(())
    }

    async fn analyze_select_query(
        &self,
        query_box: &Query,
        query: &str,
        recommendations: &mut Vec<Recommendation>,
        db_type: DatabaseType,
        profile: Profile,
    ) -> Result<()> {
        if let SetExpr::Select(select) = &*query_box.body {
            self.analyze_select_statement(select, query, recommendations, db_type, profile)
                .await?;
        }

        Ok(())
    }

    async fn analyze_select_statement(
        &self,
        select: &Select,
        query: &str,
        recommendations: &mut Vec<Recommendation>,
        db_type: DatabaseType,
        _profile: Profile,
    ) -> Result<()> {
        if select
            .projection
            .iter()
            .any(|item| matches!(item, sqlparser::ast::SelectItem::Wildcard(_)))
            && select.selection.is_none()
        {
            recommendations.push(Recommendation {
                recommendation_type: RecommendationType::QueryRewrite,
                table: None,
                columns: vec![],
                description: "SELECT * without WHERE clause may return unnecessary rows"
                    .to_string(),
                estimated_improvement: 0.1,
                sql_suggestion: Some(
                    "Consider adding specific columns and WHERE clause if not all data is needed"
                        .to_string(),
                ),
                confidence: ConfidenceTier::SyntacticGuess,
            });
        }

        if let Some(where_clause) = &select.selection {
            self.check_for_n_plus_one_patterns(where_clause, recommendations)?;
        }

        if db_type == DatabaseType::MySQL {
            self.mysql_enhanced_analysis(query, recommendations).await?;
        }

        Ok(())
    }

    fn check_for_n_plus_one_patterns(
        &self,
        where_clause: &sqlparser::ast::Expr,
        recommendations: &mut Vec<Recommendation>,
    ) -> Result<()> {
        match where_clause {
            sqlparser::ast::Expr::InSubquery { .. } => {
                recommendations.push(Recommendation {
                    recommendation_type: RecommendationType::NPlusOneQuery,
                    table: None,
                    columns: vec![],
                    description: "IN subquery detected - consider using JOIN instead".to_string(),
                    estimated_improvement: 0.5,
                    sql_suggestion: Some(
                        "Replace IN subquery with INNER JOIN for better performance".to_string(),
                    ),
                    confidence: ConfidenceTier::SyntacticGuess,
                });
            }
            sqlparser::ast::Expr::BinaryOp {
                left,
                op: sqlparser::ast::BinaryOperator::Eq,
                right,
            } if self.is_correlated_subquery(left) || self.is_correlated_subquery(right) => {
                recommendations.push(Recommendation {
                    recommendation_type: RecommendationType::NPlusOneQuery,
                    table: None,
                    columns: vec![],
                    description: "Correlated subquery detected - consider using JOIN instead"
                        .to_string(),
                    estimated_improvement: 0.6,
                    sql_suggestion: Some(
                        "Correlated subqueries are often slower than JOINs".to_string(),
                    ),
                    confidence: ConfidenceTier::SyntacticGuess,
                });
            }
            _ => {}
        }

        Ok(())
    }

    fn is_correlated_subquery(&self, expr: &sqlparser::ast::Expr) -> bool {
        matches!(expr, sqlparser::ast::Expr::Subquery(_))
    }

    async fn mysql_enhanced_analysis(
        &self,
        query: &str,
        recommendations: &mut Vec<Recommendation>,
    ) -> Result<()> {
        if query.to_lowercase().contains("select *") {
            recommendations.push(Recommendation {
                recommendation_type: RecommendationType::QueryRewrite,
                table: None,
                columns: vec![],
                description: "SELECT * in MySQL can be slower than explicit column lists"
                    .to_string(),
                estimated_improvement: 0.2,
                sql_suggestion: Some(
                    "Specify only the columns you need instead of SELECT *".to_string(),
                ),
                confidence: ConfidenceTier::SyntacticGuess,
            });
        }

        Ok(())
    }
}

impl Default for SqlAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}
