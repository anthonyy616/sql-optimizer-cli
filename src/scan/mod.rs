//! Phase 5: Project-wide scanning & multi-source ingestion.
//!
//! One `SourceExtractor` trait, multiple implementations. Each extractor yields
//! `(query_text, origin)` pairs that are routed through the same analysis
//! pipeline used by `analyze`/`batch` and deduplicated via Phase 1.5
//! fingerprinting. No new analysis logic is needed per source type.

pub mod extractors;
pub mod report;

use std::path::Path;

/// Where an extracted query came from.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Origin {
    pub file: String,
    pub line: usize,
    pub source_type: &'static str,
}

impl From<&Origin> for crate::core::annotations::Origin {
    fn from(o: &Origin) -> Self {
        Self {
            file: o.file.clone(),
            line: o.line,
            source_type: o.source_type.to_string(),
        }
    }
}

/// A query extracted from a project source.
#[derive(Debug, Clone)]
pub struct ExtractedQuery {
    pub text: String,
    pub origin: Origin,
}

/// Common extraction interface (design decision 15).
pub trait SourceExtractor {
    /// Human-readable source type name ("sql", "dbt", "app-source", "slow-log").
    fn name(&self) -> &'static str;

    /// File extensions / names this extractor handles (lowercase, without dot).
    fn handles(&self, path: &Path) -> bool;

    /// Extract queries from file content.
    fn extract(&self, path: &Path, content: &str) -> Vec<ExtractedQuery>;
}

/// Directories never descended into during a project scan.
const SKIPPED_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "target",
    "node_modules",
    "vendor",
    "dist",
    "build",
    "__pycache__",
    ".venv",
    "venv",
    "output",
];

/// Walk a path (file or directory), extracting queries from every file a
/// registered extractor claims. Never prompts; silently skips unreadable files.
pub struct Scanner {
    extractors: Vec<Box<dyn SourceExtractor>>,
    pub exclude: Vec<String>,
    pub max_file_bytes: u64,
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}

impl Scanner {
    pub fn new() -> Self {
        Self {
            extractors: vec![
                Box::new(extractors::SqlFileExtractor),
                Box::new(extractors::DbtExtractor),
                Box::new(extractors::AppSourceExtractor),
                Box::new(extractors::SlowLogExtractor),
            ],
            exclude: Vec::new(),
            max_file_bytes: 2 * 1024 * 1024,
        }
    }

    pub fn with_exclude(mut self, patterns: Vec<String>) -> Self {
        self.exclude = patterns;
        self
    }

    fn is_excluded(&self, path: &Path) -> bool {
        let p = path.to_string_lossy();
        self.exclude
            .iter()
            .any(|pat| !pat.is_empty() && p.contains(pat.as_str()))
    }

    fn pick_extractor(&self, path: &Path) -> Option<&dyn SourceExtractor> {
        // dbt models are .sql too — the dbt extractor claims them by directory
        // convention first so Jinja gets stripped before SQL splitting.
        if extractors::is_dbt_model_path(path) {
            return Some(self.extractors.iter().find(|e| e.name() == "dbt")?.as_ref());
        }
        if extractors::looks_like_slow_log_path(path) {
            return Some(
                self.extractors
                    .iter()
                    .find(|e| e.name() == "slow-log")?
                    .as_ref(),
            );
        }
        self.extractors
            .iter()
            .filter(|e| e.name() != "dbt" && e.name() != "slow-log")
            .map(|e| e.as_ref())
            .find(|e| e.handles(path))
    }

    /// Scan a file or recursively scan a directory tree.
    pub fn scan(&self, root: &Path) -> anyhow::Result<Vec<ExtractedQuery>> {
        let mut out = Vec::new();

        if root.is_file() {
            self.scan_file(root, &mut out)?;
            return Ok(out);
        }

        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let entries = match std::fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let file_name = entry.file_name().to_string_lossy().to_string();

                if path.is_dir() {
                    if !SKIPPED_DIRS.contains(&file_name.as_str()) && !self.is_excluded(&path) {
                        stack.push(path);
                    }
                    continue;
                }

                if self.is_excluded(&path) || !path.is_file() {
                    continue;
                }
                self.scan_file(&path, &mut out)?;
            }
        }

        Ok(out)
    }

    fn scan_file(&self, path: &Path, out: &mut Vec<ExtractedQuery>) -> anyhow::Result<()> {
        let extractor = match self.pick_extractor(path) {
            Some(e) => e,
            None => return Ok(()),
        };

        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(_) => return Ok(()),
        };
        if meta.len() > self.max_file_bytes {
            return Ok(());
        }
        // Skip binary-looking files.
        if is_likely_binary(path) {
            return Ok(());
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return Ok(()), // non-UTF8: skip rather than fail the whole scan
        };

        let file_str = path.to_string_lossy().replace('\\', "/");
        let mut extracted = extractor.extract(path, &content);
        for q in &mut extracted {
            q.origin.file = file_str.clone();
        }
        out.append(&mut extracted);
        Ok(())
    }
}

fn is_likely_binary(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase();
    matches!(
        ext.as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "ico"
            | "pdf"
            | "zip"
            | "gz"
            | "tar"
            | "exe"
            | "so"
            | "dylib"
            | "dll"
            | "bin"
            | "class"
            | "jar"
            | "woff"
            | "woff2"
            | "ttf"
    )
}
