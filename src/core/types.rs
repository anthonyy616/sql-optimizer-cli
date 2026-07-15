use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq, Copy, Clone)]
pub enum DatabaseType {
    PostgreSQL,
    MySQL,
    SQLite,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SchemaSnapshot {
    pub tables: Vec<TableSchema>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TableSchema {
    pub name: String,
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<IndexInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IndexInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub is_unique: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QueryPlan {
    pub engine: String,
    pub root: Option<QueryPlanNode>,
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QueryPlanNode {
    pub node_type: String,
    pub cost: Option<f64>,
    pub rows: Option<f64>,
    pub index_used: Option<String>,
    pub children: Vec<QueryPlanNode>,
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
    pub schema_snapshot: Option<SchemaSnapshot>,
    pub explain_plan: Option<QueryPlan>,
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
