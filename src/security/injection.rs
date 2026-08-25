use crate::core::types::{SecurityIssue, SecurityIssueType, Severity};
use regex::Regex;

/// Detect SQL injection risks: string concatenation, unparameterized user input patterns,
/// dangerous stored procedures, and unsanitized dynamic SQL.
pub fn detect_injection_risks(query: &str) -> Vec<SecurityIssue> {
    let mut issues = Vec::new();
    let query_lower = query.to_lowercase();

    // 1. String concatenation patterns — the most common injection vector
    check_pattern(
        &mut issues,
        query,
        r#"\+\s*['"]"#,
        "string concatenation with '+' operator",
    );
    check_pattern(
        &mut issues,
        query,
        r"CONCAT\(",
        "CONCAT() function with multiple arguments",
    );
    check_pattern(
        &mut issues,
        query,
        r"\|\|",
        "string concatenation with '||' operator",
    );

    // 2. EXEC / EXECUTE with dynamic strings
    check_contains(
        &mut issues,
        &query_lower,
        "exec(",
        "EXEC with dynamic string",
    );
    check_contains(
        &mut issues,
        &query_lower,
        "execute(",
        "EXECUTE with dynamic string",
    );
    check_contains(
        &mut issues,
        &query_lower,
        "sp_executesql",
        "sp_executesql stored procedure",
    );

    // 3. UNION-based injection indicators
    if query_lower.contains("union") && query_lower.contains("select") {
        issues.push(SecurityIssue {
            issue_type: SecurityIssueType::SqlInjection,
            description: "UNION SELECT detected — verify this is intentional and not a UNION-based injection vector".to_string(),
            severity: Severity::Medium,
            location: None,
        });
    }

    // 4. Stacked queries (multiple statements separated by semicolons)
    let semicolon_count = query.matches(';').count();
    if semicolon_count > 1 || (semicolon_count == 1 && !query.trim().ends_with(';')) {
        let trimmed = query.trim().trim_end_matches(';');
        if trimmed.contains(';') {
            issues.push(SecurityIssue {
                issue_type: SecurityIssueType::SqlInjection,
                description: "Multiple SQL statements detected (stacked queries) — may allow injection via semicolons".to_string(),
                severity: Severity::Medium,
                location: None,
            });
        }
    }

    // 5. Dangerous DDL/DML in unexpected contexts
    let dangerous_ddl: [(&str, Severity); 5] = [
        ("drop table", Severity::Critical),
        ("drop database", Severity::Critical),
        ("truncate table", Severity::High),
        ("grant ", Severity::High),
        ("revoke ", Severity::Medium),
    ];

    for (pattern, severity) in &dangerous_ddl {
        if query_lower.starts_with(pattern) || query_lower.contains(&format!("\n{}", pattern)) {
            issues.push(SecurityIssue {
                issue_type: SecurityIssueType::PrivilegeEscalation,
                description: format!("Potentially dangerous DDL/privilege operation: {}", pattern),
                severity: severity.clone(),
                location: None,
            });
        }
    }

    // 6. Hardcoded credentials or secrets in query text
    check_pattern(
        &mut issues,
        query,
        r#"(?i)password\s*=\s*'[^']+'"#,
        "hardcoded password in query",
    );
    check_pattern(
        &mut issues,
        query,
        r#"(?i)secret\s*=\s*"[^"]+""#,
        "hardcoded secret in query",
    );
    check_pattern(
        &mut issues,
        query,
        r#"(?i)api_?key\s*=\s*"[^"]+""#,
        "hardcoded API key in query",
    );

    issues
}

fn check_pattern(issues: &mut Vec<SecurityIssue>, query: &str, pattern: &str, desc: &str) {
    if let Ok(re) = Regex::new(pattern) {
        if re.is_match(query) {
            issues.push(SecurityIssue {
                issue_type: SecurityIssueType::SqlInjection,
                description: format!(
                    "Potential SQL injection: {} detected — use parameterized queries instead",
                    desc
                ),
                severity: Severity::High,
                location: None,
            });
        }
    }
}

fn check_contains(issues: &mut Vec<SecurityIssue>, query_lower: &str, pattern: &str, desc: &str) {
    if query_lower.contains(pattern) {
        issues.push(SecurityIssue {
            issue_type: SecurityIssueType::SqlInjection,
            description: format!(
                "Dangerous pattern detected: {} — ensure input is parameterized",
                desc
            ),
            severity: Severity::Critical,
            location: None,
        });
    }
}
