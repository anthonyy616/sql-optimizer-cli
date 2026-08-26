use std::path::Path;

use super::{ExtractedQuery, Origin, SourceExtractor};

/// Raw `.sql` files: split into individual statements.
pub struct SqlFileExtractor;

impl SourceExtractor for SqlFileExtractor {
    fn name(&self) -> &'static str {
        "sql"
    }

    fn handles(&self, path: &Path) -> bool {
        matches!(
            path.extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default()
                .to_lowercase()
                .as_str(),
            "sql"
        )
    }

    fn extract(&self, _path: &Path, content: &str) -> Vec<ExtractedQuery> {
        split_sql_statements(content)
            .into_iter()
            .map(|(line, text)| ExtractedQuery {
                text,
                origin: Origin {
                    file: String::new(),
                    line,
                    source_type: self.name(),
                },
            })
            .collect()
    }
}

/// dbt models (`.sql` under a `models/` directory): strip Jinja
/// `{{ ref(...) }}` / `{{ source(...) }}` / `{% ... %}` best-effort before
/// splitting. Macros requiring the dbt compiler are not resolved — documented.
pub struct DbtExtractor;

pub fn is_dbt_model_path(path: &Path) -> bool {
    let ext_matches = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("sql"))
        .unwrap_or(false);
    ext_matches && path.components().any(|c| c.as_os_str() == "models")
}

impl SourceExtractor for DbtExtractor {
    fn name(&self) -> &'static str {
        "dbt"
    }

    fn handles(&self, path: &Path) -> bool {
        is_dbt_model_path(path)
    }

    fn extract(&self, _path: &Path, content: &str) -> Vec<ExtractedQuery> {
        let stripped = strip_jinja(content);
        split_sql_statements(&stripped)
            .into_iter()
            .map(|(line, mut text)| {
                // Note unresolved Jinja rather than silently dropping it.
                if stripped.contains("{{") || stripped.contains("{%") {
                    text.push_str(
                        "\n-- note: contains Jinja that requires the dbt compiler to fully resolve",
                    );
                }
                ExtractedQuery {
                    text,
                    origin: Origin {
                        file: String::new(),
                        line,
                        source_type: self.name(),
                    },
                }
            })
            .collect()
    }
}

/// Application source files: heuristic extraction of string literals that look
/// like SQL (language-agnostic regex pass — deliberately not a real parser).
pub struct AppSourceExtractor;

const APP_SOURCE_EXTS: &[&str] = &[
    "rb", "py", "js", "jsx", "ts", "tsx", "go", "java", "rs", "php", "kt", "swift", "cs", "scala",
    "ex", "erl",
];

pub fn looks_like_app_source(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase();
    APP_SOURCE_EXTS.contains(&ext.as_str())
}

/// Slow-query / general query logs are usually `.log` or named like one.
pub fn looks_like_slow_log_path(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_lowercase();
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_lowercase();
    ext == "log" || name.contains("slow") && ext != "sql" || name.contains("query.log")
}

impl SourceExtractor for AppSourceExtractor {
    fn name(&self) -> &'static str {
        "app-source"
    }

    fn handles(&self, path: &Path) -> bool {
        looks_like_app_source(path)
    }

    fn extract(&self, _path: &Path, content: &str) -> Vec<ExtractedQuery> {
        let mut out = Vec::new();

        // Triple-quoted Python strings and Ruby heredoc-adjacent literals first,
        // then single-line quoted strings.
        for re_def in [
            r#"(?is)"""(.*?)""""#,
            r#"(?is)'''(.*?)'''"#,
            r#"(?is)"([^"\n]{10,}?)""#,
            r#"(?is)'([^'\n]{10,}?)'"#,
            r"(?is)`([^`\n]{10,}?)`",
        ] {
            let re = match regex::Regex::new(re_def) {
                Ok(r) => r,
                Err(_) => continue,
            };
            for caps in re.captures_iter(content) {
                let whole = match caps.get(0) {
                    Some(m) => m,
                    None => continue,
                };
                let literal = match caps.get(1) {
                    Some(m) => m.as_str(),
                    None => continue,
                };
                if !looks_like_sql(literal) {
                    continue;
                }
                let line = content[..whole.start()].matches('\n').count() + 1;
                let text = literal.trim().to_string();
                if !out.iter().any(|q: &ExtractedQuery| q.text == text) {
                    out.push(ExtractedQuery {
                        text,
                        origin: Origin {
                            file: String::new(),
                            line,
                            source_type: self.name(),
                        },
                    });
                }
            }
        }

        out
    }
}

/// Postgres / MySQL slow-query and general log formats.
pub struct SlowLogExtractor;

impl SourceExtractor for SlowLogExtractor {
    fn name(&self) -> &'static str {
        "slow-log"
    }

    fn handles(&self, path: &Path) -> bool {
        looks_like_slow_log_path(path)
    }

    fn extract(&self, _path: &Path, content: &str) -> Vec<ExtractedQuery> {
        let mut out = Vec::new();
        let lines: Vec<&str> = content.lines().collect();

        let pg_stmt =
            regex::Regex::new(r"(?i)duration:\s*[\d.]+\s*ms\s+(?:plan|statement):\s*(.+)$").ok();
        let mysql_time = regex::Regex::new(r"(?i)^Query_time").ok();

        let mut i = 0usize;
        while i < lines.len() {
            let line = lines[i];

            // Postgres: `2026-01-01 00:00:00 UTC [123] LOG:  duration: 12.3 ms  statement: SELECT ...`
            if let Some(re) = &pg_stmt {
                if let Some(caps) = re.captures(line) {
                    let stmt_start = caps.get(1).map(|m| m.start()).unwrap_or(0);
                    let line_no = content[..stmt_start].matches('\n').count() + 1;
                    let mut text = caps.get(1).map(|m| m.as_str()).unwrap_or("").to_string();
                    // Statement may continue on following lines until blank line.
                    let mut j = i + 1;
                    while j < lines.len()
                        && !lines[j].trim().is_empty()
                        && !lines[j].contains("LOG:")
                        && !lines[j].contains('#')
                    {
                        text.push(' ');
                        text.push_str(lines[j].trim());
                        j += 1;
                    }
                    push_unique(&mut out, normalize_sql(&text), line_no, self.name());
                    i = j;
                    continue;
                }
            }

            // MySQL slow log: header block starting with `# Time:` ... `# Query_time:`
            // followed by the statement on subsequent lines.
            if line.starts_with("# Time:") {
                // find the end of this header block
                let mut j = i + 1;
                while j < lines.len() && lines[j].starts_with('#') {
                    j += 1;
                }
                let is_mysql_block = lines[i..j].iter().any(|l| {
                    mysql_time
                        .as_ref()
                        .map(|re| re.is_match(l))
                        .unwrap_or(false)
                });
                if is_mysql_block {
                    let mut text = String::new();
                    while j < lines.len() && !lines[j].starts_with('#') {
                        text.push_str(lines[j]);
                        text.push(' ');
                        j += 1;
                    }
                    let line_no = i + 1;
                    push_unique(&mut out, normalize_sql(&text), line_no, self.name());
                    i = j;
                    continue;
                }
            }

            i += 1;
        }

        out
    }
}

fn push_unique(
    out: &mut Vec<ExtractedQuery>,
    text: String,
    line: usize,
    source_type: &'static str,
) {
    if text.is_empty() {
        return;
    }
    if !out.iter().any(|q: &ExtractedQuery| q.text == text) {
        out.push(ExtractedQuery {
            text,
            origin: Origin {
                file: String::new(),
                line,
                source_type,
            },
        });
    }
}

/// Heuristic: does this string literal look like a SQL statement?
pub fn looks_like_sql(s: &str) -> bool {
    let t = s.trim_start();
    let lower = t.to_lowercase();
    if !(lower.starts_with("select ")
        || lower.starts_with("insert ")
        || lower.starts_with("update ")
        || lower.starts_with("delete ")
        || lower.starts_with("with "))
    {
        return false;
    }
    // Require at least one SQL keyword beyond the leading verb to cut false positives.
    [
        " from ", " where ", " into ", " set ", " join ", " values ", " group ", " order ",
        " table ",
    ]
    .iter()
    .any(|kw| lower.contains(kw))
}

/// Best-effort Jinja stripping for dbt models:
/// - `{{ ref('x') }}` → `x`, `{{ source('a','b') }}` → `a_b`
/// - `{{ config(...) }}` → removed
/// - `{% ... %}` blocks → removed entirely (best-effort; compiler-level logic
///   such as loops cannot be expanded without running dbt)
pub fn strip_jinja(content: &str) -> String {
    let expr_re = regex::Regex::new(r"\{\{\s*(ref|source|var)\s*\(([^}]*)\)\s*\}\}").ok();
    let config_re = regex::Regex::new(r"(?s)\{\{[^}]*config[^}]*\}\}").ok();
    let block_re = regex::Regex::new(r"(?s)\{%.*?%\}").ok();

    let mut s = content.to_string();
    if let Some(config_re) = &config_re {
        s = config_re.replace_all(&s, "").to_string();
    }
    if let Some(block_re) = &block_re {
        s = block_re.replace_all(&s, "").to_string();
    }
    if let Some(expr_re) = &expr_re {
        s = expr_re
            .replace_all(&s, |caps: &regex::Captures| {
                let kind = caps.get(1).map(|m| m.as_str()).unwrap_or("");
                let args = caps.get(2).map(|m| m.as_str()).unwrap_or("");
                let parts: Vec<String> = args
                    .split(',')
                    .map(|p| p.trim().trim_matches(|c| c == '\'' || c == '"').to_string())
                    .filter(|p| !p.is_empty())
                    .collect();
                if kind == "var" {
                    format!("__dbt_var_{}__", parts.join("_"))
                } else {
                    parts.join("__")
                }
            })
            .to_string();
    }
    s
}

/// Split file content into statements at top-level semicolons.
/// Returns `(start_line, statement_text)` pairs; strips `--` comments that sit
/// on their own lines but preserves inline comment text inside statements.
pub fn split_sql_statements(content: &str) -> Vec<(usize, String)> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut start_line = 1usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut line = 1usize;

    let chars: Vec<char> = content.chars().collect();
    let mut i = 0usize;

    while i < chars.len() {
        let c = chars[i];
        match c {
            '\n' => {
                line += 1;
                current.push(c);
            }
            '\'' if !in_double => {
                in_single = !in_single;
                current.push(c);
            }
            '"' if !in_single => {
                in_double = !in_double;
                current.push(c);
            }
            ';' if !in_single && !in_double => {
                let text = normalize_sql(&current);
                let sl = start_line;
                if !text.is_empty() {
                    statements.push((sl, text));
                }
                current.clear();
                start_line = line + 1;
            }
            '-' if !in_single && !in_double && i + 1 < chars.len() && chars[i + 1] == '-' => {
                // Line comment: skip to end of line (keep the newline bookkeeping).
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '/' if !in_single && !in_double && i + 1 < chars.len() && chars[i + 1] == '*' => {
                // Block comment: skip to closing */
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    if chars[i] == '\n' {
                        line += 1;
                    }
                    i += 1;
                }
                i += 2;
                continue;
            }
            _ => current.push(c),
        }
        i += 1;
    }

    let text = normalize_sql(&current);
    if !text.is_empty() {
        statements.push((start_line, text));
    }
    statements
}

fn normalize_sql(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}
