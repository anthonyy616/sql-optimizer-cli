use serde::{Deserialize, Serialize};
use clap::ValueEnum;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DatabaseType {
    PostgreSQL,
    MySQL,
}

#[derive(Debug, Clone, Serialize, Deserialize, ValueEnum, Default)]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
    Yaml,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult {
    pub query: String,
    pub database_type: DatabaseType,
    pub recommendations: Vec<Recommendation>,
    pub security_score: f64,
    pub security_issues: Vec<SecurityIssue>,
    pub execution_time_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recommendation {
    pub recommendation_type: RecommendationType,
    pub table: Option<String>,
    pub columns: Vec<String>,
    pub description: String,
    pub estimated_improvement: f64,
    pub sql_suggestion: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecommendationType {
    MissingIndex,
    NPlusOneQuery,
    InefficientJoin,
    CartesianProduct,
    QueryRewrite,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityIssue {
    pub issue_type: SecurityIssueType,
    pub description: String,
    pub severity: Severity,
    pub location: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SecurityIssueType {
    SqlInjection,
    SensitiveDataExposure,
    PrivilegeEscalation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}
