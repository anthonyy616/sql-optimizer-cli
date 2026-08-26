use anyhow::{Context, Result};
use std::path::Path;

use crate::core::types::AnalysisResult;

/// A stored baseline: a JSON array of previous `AnalysisResult` runs.
#[derive(Debug, Clone, Default)]
pub struct Baseline {
    pub results: Vec<AnalysisResult>,
}

impl Baseline {
    pub fn load(path: &Path) -> Result<Baseline> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read baseline file {}", path.display()))?;
        let results: Vec<AnalysisResult> = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse baseline file {} as JSON", path.display()))?;
        Ok(Baseline { results })
    }

    /// Save a set of results as the new baseline (used by `--save-baseline`).
    pub fn save(path: &Path, results: &[AnalysisResult]) -> Result<()> {
        let json = serde_json::to_string_pretty(results)?;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::write(path, json)
            .with_context(|| format!("Failed to write baseline file {}", path.display()))?;
        Ok(())
    }

    fn finding_keys(result: &AnalysisResult) -> Vec<String> {
        let fp = crate::core::fingerprint::fingerprint(&result.query);
        let mut keys = Vec::new();
        for issue in &result.security_issues {
            keys.push(format!(
                "sec|{}|{}|{}",
                fp,
                issue.issue_type_name(),
                issue.description
            ));
        }
        for rec in &result.recommendations {
            keys.push(format!(
                "rec|{}|{}|{}",
                fp,
                rec.recommendation_type.name(),
                rec.description
            ));
        }
        keys
    }

    /// Filter current results down to findings that are NOT present in the
    /// baseline — i.e. only new regressions/issues are reported. Results whose
    /// query is entirely unknown to the baseline keep all their findings.
    pub fn filter_new_findings(&self, current: &[AnalysisResult]) -> Vec<AnalysisResult> {
        let known_keys: std::collections::HashSet<String> =
            self.results.iter().flat_map(Self::finding_keys).collect();

        current
            .iter()
            .map(|result| {
                let mut filtered = result.clone();
                filtered.recommendations.retain(|rec| {
                    !known_keys.contains(&format!(
                        "rec|{}|{}|{}",
                        crate::core::fingerprint::fingerprint(&result.query),
                        rec.recommendation_type.name(),
                        rec.description
                    ))
                });
                filtered.security_issues.retain(|issue| {
                    !known_keys.contains(&format!(
                        "sec|{}|{}|{}",
                        crate::core::fingerprint::fingerprint(&result.query),
                        issue.issue_type_name(),
                        issue.description
                    ))
                });
                filtered
            })
            .collect()
    }
}
