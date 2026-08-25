use crate::core::types::{SchemaSnapshot, SecurityIssue, Severity};

/// Compute an aggregate security score from a list of issues.
/// 100.0 = no issues; lower = worse. Each issue deducts based on severity.
pub fn compute_security_score(issues: &[SecurityIssue]) -> f64 {
    let mut score = 100.0_f64;
    for issue in issues {
        let deduction = match issue.severity {
            Severity::Low => 5.0,
            Severity::Medium => 15.0,
            Severity::High => 30.0,
            Severity::Critical => 50.0,
        };
        score -= deduction;
    }
    score.max(0.0)
}

/// Run all security checks against a query and schema, returning
/// deduplicated issues and an aggregate score.
pub fn validate_security(query: &str, schema: &SchemaSnapshot) -> (f64, Vec<SecurityIssue>) {
    let mut all_issues = Vec::new();

    // Injection risk checks
    let mut injection_issues = super::injection::detect_injection_risks(query);
    all_issues.append(&mut injection_issues);

    // Sensitive data exposure checks
    let mut sensitive_issues = super::sensitive_data::detect_sensitive_data(query, schema);
    all_issues.append(&mut sensitive_issues);

    // Privilege overreach checks (currently returns empty — TODO)
    let mut privilege_issues = super::sensitive_data::check_privilege_overreach(query, schema);
    all_issues.append(&mut privilege_issues);

    // Deduplicate issues by (type, location)
    all_issues.sort_by(|a, b| {
        format!("{:?}:{:?}", a.issue_type, a.location)
            .cmp(&format!("{:?}:{:?}", b.issue_type, b.location))
    });
    all_issues.dedup_by(|a, b| a.issue_type == b.issue_type && a.location == b.location);

    let score = compute_security_score(&all_issues);
    (score, all_issues)
}
