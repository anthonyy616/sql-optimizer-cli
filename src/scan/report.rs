use std::collections::HashMap;

use crate::core::fingerprint;
use crate::core::types::AnalysisResult;

use super::Origin;

/// One scanned (and optionally analyzed) query.
#[derive(serde::Serialize)]
pub struct ScanEntry {
    pub query: String,
    pub fingerprint: String,
    pub origin: Origin,
    /// Present when a database was connected for the scan; absent for pure
    /// static scans.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<AnalysisResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Aggregated view of a scan, grouped by query fingerprint (Phase 1.5).
#[derive(serde::Serialize)]
pub struct ScanReport {
    pub root: String,
    pub files_scanned_hint: usize,
    pub total_queries_extracted: usize,
    pub unique_shapes: usize,
    pub entries: Vec<ScanEntry>,
    /// Top offender groups ordered by occurrence count, then worst finding count.
    pub top_offenders: Vec<OffenderGroup>,
}

#[derive(serde::Serialize)]
pub struct OffenderGroup {
    pub fingerprint: String,
    pub occurrences: usize,
    pub example_query: String,
    pub origins: Vec<Origin>,
    pub findings: usize,
    pub worst_severity: Option<String>,
}

/// Build the grouped report from raw extractions. `analyze_one` runs the
/// standard analyze pipeline per *unique shape* (not per occurrence), which is
/// what makes scanning a large project cheap.
pub fn build_report<F>(
    root: &str,
    extracted: Vec<(String, Origin)>,
    mut analyze_one: F,
) -> ScanReport
where
    F: FnMut(&str) -> Option<AnalysisResult>,
{
    let total = extracted.len();

    // Group occurrences by fingerprint.
    let mut groups: HashMap<String, Vec<(String, Origin)>> = HashMap::new();
    for (text, origin) in extracted {
        let fp = fingerprint::fingerprint(&text);
        groups.entry(fp).or_default().push((text, origin));
    }

    let mut entries = Vec::new();
    let mut offenders = Vec::new();

    for (fp, mut occurrences) in groups {
        occurrences.sort_by_key(|a| a.1.line);
        let (example_text, _) = occurrences[0].clone();

        let result = analyze_one(&example_text);

        let findings = result.as_ref().map(count_findings).unwrap_or(0);
        let worst = result
            .as_ref()
            .and_then(|r| r.max_severity().map(|s| s.as_str().to_string()));

        offenders.push(OffenderGroup {
            fingerprint: fp.clone(),
            occurrences: occurrences.len(),
            example_query: example_text.clone(),
            origins: occurrences.iter().map(|(_, o)| o.clone()).collect(),
            findings,
            worst_severity: worst,
        });

        // First occurrence carries the analysis; the rest are listed as origins only.
        for (idx, (text, origin)) in occurrences.into_iter().enumerate() {
            entries.push(ScanEntry {
                query: text,
                fingerprint: fp.clone(),
                origin,
                result: if idx == 0 { result.clone() } else { None },
                error: None,
            });
        }
    }

    offenders.sort_by(|a, b| {
        b.occurrences
            .cmp(&a.occurrences)
            .then(b.findings.cmp(&a.findings))
    });

    ScanReport {
        root: root.to_string(),
        files_scanned_hint: 0,
        total_queries_extracted: total,
        unique_shapes: entries
            .iter()
            .map(|e| e.fingerprint.clone())
            .collect::<Vec<_>>()
            .len(),
        entries,
        top_offenders: offenders,
    }
}

fn count_findings(result: &AnalysisResult) -> usize {
    result.security_issues.len()
        + result.recommendations.len()
        + result.regressions.len()
        + result.schema_drift.len()
}

impl ScanReport {
    /// All results with findings, suitable for annotation emission.
    pub fn results_with_origins(
        &self,
    ) -> Vec<(Option<crate::core::annotations::Origin>, &AnalysisResult)> {
        self.entries
            .iter()
            .filter_map(|entry| {
                entry.result.as_ref().filter(|r| r.has_findings()).map(|r| {
                    (
                        Some(crate::core::annotations::Origin::from(&entry.origin)),
                        r,
                    )
                })
            })
            .collect()
    }

    /// Worst severity across every analyzed entry.
    pub fn max_severity(&self) -> Option<crate::core::types::Severity> {
        self.entries
            .iter()
            .filter_map(|e| e.result.as_ref())
            .filter_map(|r| r.max_severity())
            .max_by(|a, b| a.rank().cmp(&b.rank()))
    }

    pub fn has_findings(&self) -> bool {
        self.entries
            .iter()
            .any(|e| e.result.as_ref().map(|r| r.has_findings()).unwrap_or(false))
    }

    /// Render the human-readable top-offenders summary (used in text mode).
    pub fn render_summary(&self) -> String {
        use std::fmt::Write;
        let mut out = String::new();
        let _ = writeln!(
            &mut out,
            "Project scan: {} — {} queries extracted, {} unique shapes",
            self.root, self.total_queries_extracted, self.unique_shapes
        );
        let _ = writeln!(&mut out, "\nTOP OFFENDERS (by occurrences):");
        for (i, g) in self.top_offenders.iter().take(10).enumerate() {
            let _ = writeln!(
                &mut out,
                "{}. [{}x] {}",
                i + 1,
                g.occurrences,
                truncate(&g.example_query, 90)
            );
            for o in g.origins.iter().take(3) {
                let _ = writeln!(
                    &mut out,
                    "     at {}:{} ({})",
                    o.file, o.line, o.source_type
                );
            }
            let _ = writeln!(
                &mut out,
                "     findings: {}, worst severity: {}",
                g.findings,
                g.worst_severity.as_deref().unwrap_or("none")
            );
        }
        out
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{}…", t)
    }
}
