use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq, Copy, Clone)]
pub enum DatabaseType {
    PostgreSQL,
    MySQL,
    SQLite,
}

#[derive(Debug, Clone, Serialize, Deserialize, ValueEnum, Default, PartialEq, Eq)]
pub enum Profile {
    #[default]
    Oltp,
    Analytics,
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
    pub foreign_keys: Vec<ForeignKeyInfo>,
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
pub struct ForeignKeyInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub referenced_table: String,
    pub referenced_columns: Vec<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RowPreview {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub truncated: bool,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, ValueEnum, Default)]
pub enum OutputFormat {
    #[default]
    Text,
    Json,
    Yaml,
    Markdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult {
    pub query: String,
    pub database_type: DatabaseType,
    #[serde(default)]
    pub profile: Profile,
    pub recommendations: Vec<Recommendation>,
    pub security_score: f64,
    pub security_issues: Vec<SecurityIssue>,
    pub schema_snapshot: Option<SchemaSnapshot>,
    pub explain_plan: Option<QueryPlan>,
    pub row_preview: Option<RowPreview>,
    pub execution_time_ms: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regressions: Vec<RegressionInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schema_drift: Vec<SchemaDriftItem>,
}

/// A single schema change detected between a stored baseline snapshot and the live schema.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SchemaDriftItem {
    /// e.g. "index-dropped", "index-added", "column-dropped", "column-added",
    /// "column-type-changed", "table-dropped", "table-added"
    pub kind: String,
    pub table: String,
    pub detail: String,
}

/// Regression info serialized into JSON output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegressionInfo {
    pub regression_type: String,
    pub description: String,
    pub current_value: String,
    pub previous_value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recommendation {
    pub recommendation_type: RecommendationType,
    pub table: Option<String>,
    pub columns: Vec<String>,
    pub description: String,
    pub estimated_improvement: f64,
    pub sql_suggestion: Option<String>,
    pub confidence: ConfidenceTier,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConfidenceTier {
    /// Pattern detected purely from SQL syntax, no database context.
    SyntacticGuess,
    /// Verified against real database schema (indexes, columns, constraints).
    SchemaVerified,
    /// Verified against a real EXPLAIN plan output.
    PlanVerified,
    /// Detected from query shape resembling known ORM output patterns.
    /// Never elevated to schema/plan-verified on its own.
    OrmHeuristic,
}

impl std::fmt::Display for ConfidenceTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfidenceTier::SyntacticGuess => write!(f, "syntactic guess"),
            ConfidenceTier::SchemaVerified => write!(f, "schema-verified"),
            ConfidenceTier::PlanVerified => write!(f, "plan-verified"),
            ConfidenceTier::OrmHeuristic => write!(f, "orm-heuristic"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RecommendationType {
    MissingIndex,
    NPlusOneQuery,
    InefficientJoin,
    CartesianProduct,
    QueryRewrite,
}

impl RecommendationType {
    pub fn name(&self) -> &'static str {
        match self {
            RecommendationType::MissingIndex => "missing_index",
            RecommendationType::NPlusOneQuery => "n_plus_one",
            RecommendationType::InefficientJoin => "inefficient_join",
            RecommendationType::CartesianProduct => "cartesian_product",
            RecommendationType::QueryRewrite => "query_rewrite",
        }
    }
}

impl AnalysisResult {
    /// The worst severity among security issues in this result, if any.
    pub fn max_severity(&self) -> Option<Severity> {
        self.security_issues
            .iter()
            .map(|i| i.severity.clone())
            .max_by(|a, b| a.rank().cmp(&b.rank()))
    }

    /// True if the result carries any finding worth reporting.
    pub fn has_findings(&self) -> bool {
        !self.security_issues.is_empty()
            || !self.recommendations.is_empty()
            || !self.regressions.is_empty()
            || !self.schema_drift.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityIssue {
    pub issue_type: SecurityIssueType,
    pub description: String,
    pub severity: Severity,
    pub location: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SecurityIssueType {
    SqlInjection,
    SensitiveDataExposure,
    PrivilegeEscalation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

impl SecurityIssue {
    pub fn issue_type_name(&self) -> &'static str {
        match self.issue_type {
            SecurityIssueType::SqlInjection => "sql_injection",
            SecurityIssueType::SensitiveDataExposure => "sensitive_data_exposure",
            SecurityIssueType::PrivilegeEscalation => "privilege_escalation",
        }
    }
}

impl Severity {
    /// Numeric rank for comparisons (higher = more severe).
    pub fn rank(&self) -> u8 {
        match self {
            Severity::Low => 0,
            Severity::Medium => 1,
            Severity::High => 2,
            Severity::Critical => 3,
        }
    }

    /// Parse a severity name (case-insensitive). Used by `--fail-on` and config.
    pub fn parse(s: &str) -> Option<Severity> {
        match s.trim().to_lowercase().as_str() {
            "low" => Some(Severity::Low),
            "medium" => Some(Severity::Medium),
            "high" => Some(Severity::High),
            "critical" => Some(Severity::Critical),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Severity::Low => "low",
            Severity::Medium => "medium",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ConnectOptions {
    pub simple_mode: bool,
    pub connect_timeout_secs: Option<u64>,
    pub accept_invalid_certs: bool,
}
