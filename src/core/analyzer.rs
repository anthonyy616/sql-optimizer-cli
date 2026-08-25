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
        let mut security_issues = Vec::new();

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

        self.basic_security_analysis(query, &mut security_issues)?;

        let execution_time = start_time.elapsed().as_millis() as u64;

        Ok(AnalysisResult {
            query: query.to_string(),
            database_type: db_type,
            profile,
            recommendations,
            security_score: if security_issues.is_empty() {
                100.0
            } else {
                50.0
            },
            security_issues,
            schema_snapshot: None,
            explain_plan: None,
            row_preview: Default::default(),
            execution_time_ms: execution_time,
        })
    }

    pub async fn run_schema_checks(&self, result: &mut AnalysisResult) -> Result<()> {
        if let Some(schema) = &result.schema_snapshot {
            // call missing index detector
            let mut recs =
                crate::patterns::missing_index::detect_missing_index(&result.query, schema);
            result.recommendations.append(&mut recs);
        }

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

    fn basic_security_analysis(&self, query: &str, issues: &mut Vec<SecurityIssue>) -> Result<()> {
        let dangerous_patterns = [
            "union select",
            "drop table",
            "delete from",
            "insert into",
            "update set",
            "exec(",
            "execute(",
            "sp_executesql",
        ];

        let query_lower = query.to_lowercase();
        for pattern in dangerous_patterns {
            if query_lower.contains(pattern) {
                issues.push(SecurityIssue {
                    issue_type: SecurityIssueType::SqlInjection,
                    description: format!("Potentially dangerous SQL pattern detected: {}", pattern),
                    severity: Severity::Medium,
                    location: Some(format!("Contains '{}'", pattern)),
                });
            }
        }

        Ok(())
    }
}

impl Default for SqlAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}
