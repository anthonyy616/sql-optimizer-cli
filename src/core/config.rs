use anyhow::Result;
use serde::Deserialize;
use std::path::Path;

/// Layered configuration loaded from `.sql-optimizer.toml` in the working
/// directory. CLI flags always win over config values, which in turn win over
/// built-in defaults.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ToolConfig {
    /// Severity at which the run is considered blocking: low|medium|high|critical
    pub fail_on: Option<String>,
    /// Default profile: oltp|analytics
    pub profile: Option<String>,
    /// Annotation format: github|gitlab|sarif
    pub annotate: Option<String>,
    /// Default output format: text|json|yaml|markdown
    pub output: Option<String>,
    /// Glob-like substrings to exclude from `scan` (matched against file paths)
    pub exclude: Option<Vec<String>>,
}

impl ToolConfig {
    /// Load `.sql-optimizer.toml` from the given directory if it exists.
    pub fn load_from(dir: &Path) -> Result<ToolConfig> {
        let path = dir.join(".sql-optimizer.toml");
        if !path.exists() {
            return Ok(ToolConfig::default());
        }
        let content = std::fs::read_to_string(&path)?;
        let cfg = toml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse {}: {}", path.display(), e))?;
        Ok(cfg)
    }

    /// Load from the current working directory (the usual case).
    pub fn load() -> Result<ToolConfig> {
        Self::load_from(Path::new("."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_config_is_default() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = ToolConfig::load_from(tmp.path()).unwrap();
        assert!(cfg.fail_on.is_none());
        assert!(cfg.exclude.is_none());
    }

    #[test]
    fn parses_full_config() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(".sql-optimizer.toml"),
            "fail_on = \"high\"\nprofile = \"analytics\"\nannotate = \"github\"\nexclude = [\"target\", \"node_modules\"]\n",
        )
        .unwrap();
        let cfg = ToolConfig::load_from(tmp.path()).unwrap();
        assert_eq!(cfg.fail_on.as_deref(), Some("high"));
        assert_eq!(cfg.profile.as_deref(), Some("analytics"));
        assert_eq!(cfg.annotate.as_deref(), Some("github"));
        assert_eq!(
            cfg.exclude.as_deref(),
            Some(&["target".to_string(), "node_modules".to_string()][..])
        );
    }
}
