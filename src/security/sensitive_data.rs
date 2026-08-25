use crate::core::types::{SchemaSnapshot, SecurityIssue, SecurityIssueType, Severity};

/// Column name patterns that suggest sensitive/PII data.
const SENSITIVE_PATTERNS: &[(&str, &str, Severity)] = &[
    // High severity — directly identifying
    ("password", "password or credential column", Severity::High),
    ("passwd", "password or credential column", Severity::High),
    ("secret", "secret or credential column", Severity::High),
    ("token", "authentication token column", Severity::High),
    ("api_key", "API key column", Severity::High),
    ("apikey", "API key column", Severity::High),
    ("private_key", "private key column", Severity::Critical),
    // High severity — government IDs
    ("ssn", "social security number column", Severity::High),
    (
        "social_security",
        "social security number column",
        Severity::High,
    ),
    ("sin", "social insurance number column", Severity::High),
    ("passport", "passport number column", Severity::High),
    ("driver_license", "driver's license column", Severity::High),
    // Medium severity — financial
    ("credit_card", "credit card number column", Severity::High),
    ("card_number", "payment card number column", Severity::High),
    ("cvv", "card verification value column", Severity::Critical),
    ("bank_account", "bank account number column", Severity::High),
    (
        "routing_number",
        "bank routing number column",
        Severity::High,
    ),
    // Medium severity — PII
    ("email", "email address column (PII)", Severity::Medium),
    ("phone", "phone number column (PII)", Severity::Medium),
    ("address", "physical address column (PII)", Severity::Medium),
    (
        "date_of_birth",
        "date of birth column (PII)",
        Severity::Medium,
    ),
    ("dob", "date of birth column (PII)", Severity::Medium),
    ("birth_date", "date of birth column (PII)", Severity::Medium),
    ("full_name", "full name column (PII)", Severity::Medium),
    ("first_name", "first name column (PII)", Severity::Low),
    ("last_name", "last name column (PII)", Severity::Low),
    ("ip_address", "IP address column", Severity::Medium),
    (
        "geo_location",
        "geographic location column",
        Severity::Medium,
    ),
    ("salary", "salary / compensation column", Severity::Medium),
    ("income", "income column", Severity::Medium),
    ("medical", "medical data column", Severity::High),
    ("diagnosis", "medical diagnosis column", Severity::High),
    ("health_record", "health record column", Severity::High),
];

/// Detect queries that SELECT or expose columns matching sensitive data patterns.
pub fn detect_sensitive_data(query: &str, schema: &SchemaSnapshot) -> Vec<SecurityIssue> {
    let mut issues = Vec::new();
    let query_upper = query.to_uppercase();

    // Check if this is a write operation
    let is_write = query_upper.starts_with("INSERT")
        || query_upper.starts_with("UPDATE")
        || query_upper.starts_with("DELETE")
        || query_upper.starts_with("DROP")
        || query_upper.starts_with("ALTER")
        || query_upper.starts_with("CREATE");

    // Check each table's columns against sensitive patterns
    for table in &schema.tables {
        for col in &table.columns {
            let col_lower = col.name.to_lowercase();
            for (pattern, description, severity) in SENSITIVE_PATTERNS {
                if col_lower.contains(pattern) {
                    // Check if this column is referenced in the query
                    let col_upper = col.name.to_uppercase();
                    if query_upper.contains(&col_upper)
                        || query_upper.contains(&format!("\"{}\"", col.name))
                        || query_upper.contains(&format!("'{}'", col.name))
                    {
                        let context = if is_write {
                            format!(
                                "Sensitive column '{}.{}' ({}) is involved in a write operation",
                                table.name, col.name, description
                            )
                        } else {
                            format!(
                                "Query accesses sensitive column '{}.{}' ({})",
                                table.name, col.name, description
                            )
                        };

                        issues.push(SecurityIssue {
                            issue_type: SecurityIssueType::SensitiveDataExposure,
                            description: context,
                            severity: severity.clone(),
                            location: Some(format!("{}.{}", table.name, col.name)),
                        });
                        break; // One issue per column
                    }
                }
            }
        }
    }

    // Also check for SELECT * on tables with known sensitive columns
    if query_upper.contains("SELECT *") || query_upper.contains("SELECT 1") {
        for table in &schema.tables {
            let table_name_upper = table.name.to_uppercase();
            if query_upper.contains(&table_name_upper) {
                let has_sensitive = table.columns.iter().any(|col| {
                    let col_lower = col.name.to_lowercase();
                    SENSITIVE_PATTERNS
                        .iter()
                        .any(|(p, _, _)| col_lower.contains(p))
                });
                if has_sensitive {
                    issues.push(SecurityIssue {
                        issue_type: SecurityIssueType::SensitiveDataExposure,
                        description: format!(
                            "SELECT * on table '{}' which contains sensitive columns — specify only needed columns",
                            table.name
                        ),
                        severity: Severity::Medium,
                        location: Some(table.name.clone()),
                    });
                }
            }
        }
    }

    issues
}

/// Check for privilege overreach: does the connected role have more access than needed?
/// This requires grant introspection which is database-specific.
/// Returns a placeholder that can be expanded when grant introspection is added.
pub fn check_privilege_overreach(_query: &str, _schema: &SchemaSnapshot) -> Vec<SecurityIssue> {
    // Privilege overreach checks require introspecting grants:
    // - information_schema.role_table_grants (Postgres)
    // - SHOW GRANTS (MySQL, often requires elevated privilege)
    // Must degrade to "not checked" when not available.
    // TODO: implement when grant introspection is added in a future phase.
    Vec::new()
}
