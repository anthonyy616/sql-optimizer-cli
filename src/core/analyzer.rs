use anyhow::{Result, Context};
use sqlparser::ast::{Query, SetExpr, Statement, Select};
use sqlparser::dialect::{GenericDialect, PostgreSqlDialect, MySqlDialect};
use sqlparser::parser::Parser;

use crate::core::types::*;
use crate::database::connection::DatabaseConnector;

pub struct SqlAnalyzer {
    database_connector: Option<Box<dyn DatabaseConnector>>,
}

impl SqlAnalyzer {
    pub fn new() -> Self {
        Self {
            database_connector: None,
        }
    }

    pub fn with_database(mut self, connector: Box<dyn DatabaseConnector>) -> Self {
        self.database_connector = Some(connector);
        self
    }

    pub fn parse_query(&self, query: &str, dialect: &str) -> Result<Vec<Statement>> {
        let dialect = match dialect.to_lowercase().as_str() {
            "postgresql" | "postgres" => Box::new(PostgreSqlDialect {}) as Box<dyn sqlparser::dialect::Dialect>,
            "mysql" => Box::new(MySqlDialect {}) as Box<dyn sqlparser::dialect::Dialect>,
            _ => Box::new(GenericDialect {}) as Box<dyn sqlparser::dialect::Dialect>,
        };

        let statements = Parser::parse_sql(&*dialect, query)
            .with_context(|| format!("Failed to parse SQL query: {}", query))?;

        Ok(statements)
    }

    pub fn analyze_query(&self, query: &str, db_type: DatabaseType) -> Result<AnalysisResult> {
        let start_time = std::time::Instant::now();
        
        // Parse the query
        let dialect = match db_type {
            DatabaseType::PostgreSQL => "postgresql",
            DatabaseType::MySQL => "mysql",
        };
        
        let statements = self.parse_query(query, dialect)?;
        
        let mut recommendations = Vec::new();
        let mut security_issues = Vec::new();
        
        // Basic analysis for each statement
        for statement in &statements {
            if let Statement::Query(query_box) = statement {
                self.analyze_select_query(&query_box, &mut recommendations)?;
            }
        }
        
        // Basic security analysis
        self.basic_security_analysis(query, &mut security_issues)?;
        
        let execution_time = start_time.elapsed().as_millis() as u64;
        
        Ok(AnalysisResult {
            query: query.to_string(),
            database_type: db_type,
            recommendations,
            security_score: if security_issues.is_empty() { 100.0 } else { 50.0 },
            security_issues,
            execution_time_ms: execution_time,
        })
    }

    fn analyze_select_query(&self, query_box: &Query, recommendations: &mut Vec<Recommendation>) -> Result<()> {
        // Look for basic patterns that can be optimized
        if let SetExpr::Select(select) = &*query_box.body {
            self.analyze_select_statement(select, recommendations)?;
        }
        Ok(())
    }

    fn analyze_select_statement(&self, select: &Select, recommendations: &mut Vec<Recommendation>) -> Result<()> {
        // Check for SELECT * without WHERE clause
        if select.projection.iter().any(|item| matches!(item, sqlparser::ast::SelectItem::Wildcard(_))) 
            && select.selection.is_none() {
            recommendations.push(Recommendation {
                recommendation_type: RecommendationType::QueryRewrite,
                table: None,
                columns: vec![],
                description: "SELECT * without WHERE clause may return unnecessary rows".to_string(),
                estimated_improvement: 0.1,
                sql_suggestion: Some("Consider adding specific columns and WHERE clause if not all data is needed".to_string()),
            });
        }
        
        // Check for basic N+1 patterns
        if let Some(where_clause) = &select.selection {
            self.check_for_n_plus_one_patterns(where_clause, recommendations)?;
        }
        
        Ok(())
    }

    fn check_for_n_plus_one_patterns(&self, where_clause: &sqlparser::ast::Expr, recommendations: &mut Vec<Recommendation>) -> Result<()> {
        // Simple check for IN subqueries that could be JOINs
        if let sqlparser::ast::Expr::InSubquery { .. } = where_clause {
            recommendations.push(Recommendation {
                recommendation_type: RecommendationType::NPlusOneQuery,
                table: None,
                columns: vec![],
                description: "IN subquery detected - consider using JOIN instead".to_string(),
                estimated_improvement: 0.5,
                sql_suggestion: Some("Replace IN subquery with INNER JOIN for better performance".to_string()),
            });
        }
        Ok(())
    }

    fn basic_security_analysis(&self, query: &str, issues: &mut Vec<SecurityIssue>) -> Result<()> {
        // Check for obvious SQL injection patterns
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
