use crate::core::types::{AnalysisResult, Severity};

/// Where a finding came from (populated by project-wide scanning; absent for
/// single-query analyze runs).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Origin {
    pub file: String,
    pub line: usize,
    pub source_type: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationFormat {
    Github,
    Gitlab,
    Sarif,
}

impl AnnotationFormat {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "github" => Some(Self::Github),
            "gitlab" => Some(Self::Gitlab),
            "sarif" => Some(Self::Sarif),
            _ => None,
        }
    }
}

/// One flat finding extracted from an analysis result, ready for annotation.
pub struct Finding {
    pub severity: Severity,
    pub check_name: String,
    pub message: String,
    pub origin: Option<Origin>,
}

/// Collect every annotatable finding from a set of results.
pub fn collect_findings(results: &[(Option<Origin>, &AnalysisResult)]) -> Vec<Finding> {
    let mut findings = Vec::new();

    for (origin, result) in results {
        for issue in &result.security_issues {
            findings.push(Finding {
                severity: issue.severity.clone(),
                check_name: format!("{:?}", issue.issue_type),
                message: issue.description.clone(),
                origin: origin.clone(),
            });
        }
        for reg in &result.regressions {
            findings.push(Finding {
                severity: Severity::High,
                check_name: reg.regression_type.clone(),
                message: reg.description.clone(),
                origin: origin.clone(),
            });
        }
        for drift in &result.schema_drift {
            let severity =
                if drift.kind.ends_with("-dropped") || drift.kind == "column-type-changed" {
                    Severity::Medium
                } else {
                    Severity::Low
                };
            findings.push(Finding {
                severity,
                check_name: drift.kind.clone(),
                message: drift.detail.clone(),
                origin: origin.clone(),
            });
        }
        // High-impact recommendations become warnings so they surface in review.
        for rec in &result.recommendations {
            if rec.estimated_improvement >= 0.5 {
                findings.push(Finding {
                    severity: Severity::Low,
                    check_name: format!("{:?}", rec.recommendation_type),
                    message: rec.description.clone(),
                    origin: origin.clone(),
                });
            }
        }
    }

    findings
}

/// Render annotations in the requested format. Output is additive to the main
/// formatter output and is written to stdout by the caller.
pub fn render_annotations(
    results: &[(Option<Origin>, &AnalysisResult)],
    fmt: AnnotationFormat,
) -> String {
    let findings = collect_findings(results);
    match fmt {
        AnnotationFormat::Github => render_github(&findings),
        AnnotationFormat::Gitlab => render_gitlab(&findings),
        AnnotationFormat::Sarif => render_sarif(&findings),
    }
}

fn render_github(findings: &[Finding]) -> String {
    let mut out = String::new();
    for f in findings {
        let level = github_level(&f.severity);
        let mut cmd = format!("::{} ", level);
        if let Some(o) = &f.origin {
            cmd.push_str(&format!("file={},line={},", o.file, o.line));
        }
        // Escape % , \r, \n per workflow-command rules.
        let msg = f
            .message
            .replace('%', "%25")
            .replace('\r', "%0D")
            .replace('\n', "%0A");
        cmd.push_str(&format!(
            "title={}:{},{}",
            escape_github_property(&f.check_name),
            msg,
            msg
        ));
        out.push_str(&cmd);
        out.push('\n');
    }
    out
}

fn github_level(sev: &Severity) -> &'static str {
    match sev.rank() {
        3 | 2 => "error",
        1 => "warning",
        _ => "notice",
    }
}

fn escape_github_property(s: &str) -> String {
    s.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
        .replace(':', "%3A")
        .replace(',', "%2C")
}

fn gitlab_severity(sev: &Severity) -> &'static str {
    match sev {
        Severity::Critical => "critical",
        Severity::High => "major",
        Severity::Medium => "minor",
        Severity::Low => "info",
    }
}

#[derive(serde::Serialize)]
struct GitLabFinding<'a> {
    description: &'a str,
    check_name: &'a str,
    severity: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<GitLabLocation>,
    fingerprint: String,
}

#[derive(serde::Serialize)]
struct GitLabLocation {
    path: String,
    lines: GitLabLines,
}

#[derive(serde::Serialize)]
struct GitLabLines {
    begin: usize,
}

fn render_gitlab(findings: &[Finding]) -> String {
    let mapped: Vec<GitLabFinding> = findings
        .iter()
        .map(|f| GitLabFinding {
            description: &f.message,
            check_name: &f.check_name,
            severity: gitlab_severity(&f.severity),
            location: f.origin.as_ref().map(|o| GitLabLocation {
                path: o.file.clone(),
                lines: GitLabLines {
                    begin: o.line.max(1),
                },
            }),
            fingerprint: gitlab_fingerprint(f),
        })
        .collect();
    serde_json::to_string_pretty(&mapped).unwrap_or_else(|_| "[]".to_string())
}

/// Stable fingerprint so GitLab can dedupe identical findings across pipelines.
fn gitlab_fingerprint(f: &Finding) -> String {
    use sha2::{Digest, Sha256};
    let key = format!(
        "{}|{}|{}",
        f.check_name,
        f.message,
        f.origin.as_ref().map(|o| o.file.as_str()).unwrap_or("")
    );
    let hash = Sha256::digest(key.as_bytes());
    hex::encode(&hash[..16])
}

#[derive(serde::Serialize)]
struct SarifReport {
    #[serde(rename = "$schema")]
    schema: String,
    version: String,
    runs: Vec<SarifRun>,
}

#[derive(serde::Serialize)]
struct SarifRun {
    tool: SarifTool,
    results: Vec<SarifResult>,
}

#[derive(serde::Serialize)]
struct SarifTool {
    driver: SarifDriver,
}

#[derive(serde::Serialize)]
struct SarifDriver {
    name: String,
    version: String,
}

#[derive(serde::Serialize)]
struct SarifResult {
    rule_id: String,
    level: &'static str,
    message: SarifMessage,
    #[serde(skip_serializing_if = "Option::is_none")]
    locations: Option<Vec<SarifLocation>>,
}

#[derive(serde::Serialize)]
struct SarifMessage {
    text: String,
}

#[derive(serde::Serialize)]
struct SarifLocation {
    physical_location: SarifPhysicalLocation,
}

#[derive(serde::Serialize)]
struct SarifPhysicalLocation {
    artifact_location: SarifArtifactLocation,
    region: SarifRegion,
}

#[derive(serde::Serialize)]
struct SarifArtifactLocation {
    uri: String,
}

#[derive(serde::Serialize)]
struct SarifRegion {
    start_line: usize,
}

fn sarif_level(sev: &Severity) -> &'static str {
    match sev.rank() {
        3 | 2 => "error",
        1 => "warning",
        _ => "note",
    }
}

fn render_sarif(findings: &[Finding]) -> String {
    let results: Vec<SarifResult> = findings
        .iter()
        .map(|f| SarifResult {
            rule_id: format!("sql-optimizer/{}", f.check_name),
            level: sarif_level(&f.severity),
            message: SarifMessage {
                text: f.message.clone(),
            },
            locations: f.origin.as_ref().map(|o| {
                vec![SarifLocation {
                    physical_location: SarifPhysicalLocation {
                        artifact_location: SarifArtifactLocation {
                            uri: o.file.clone(),
                        },
                        region: SarifRegion {
                            start_line: o.line.max(1),
                        },
                    },
                }]
            }),
        })
        .collect();

    let report = SarifReport {
        schema: "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/master/Schemata/sarif-schema-2.1.0.json".to_string(),
        version: "2.1.0".to_string(),
        runs: vec![SarifRun {
            tool: SarifTool {
                driver: SarifDriver {
                    name: "sql-optimizer-cli".to_string(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                },
            },
            results,
        }],
    };

    serde_json::to_string_pretty(&report)
        .unwrap_or_else(|_| "{\"version\":\"2.1.0\",\"runs\":[]}".to_string())
}
