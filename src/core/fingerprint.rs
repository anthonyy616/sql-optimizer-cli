use regex::Regex;
use sha2::{Digest, Sha256};

/// Canonicalize a SQL query by removing literal values and normalizing whitespace/casing.
pub fn canonicalize_query(query: &str) -> String {
    // Replace single-quoted and double-quoted string literals with ?
    let s = query.to_string();
    let re_str = Regex::new(r#"'(?:''|[^'])*'|\"(?:\"\"|[^"])*\""#).unwrap();
    let without_strings = re_str.replace_all(&s, "?");

    // Replace numeric literals (simple heuristic)
    let re_num = Regex::new(r"\b\d+(?:\.\d+)?\b").unwrap();
    let without_numbers = re_num.replace_all(&without_strings, "?");

    // Normalize whitespace and casing
    let ws = Regex::new(r"\s+").unwrap();
    let lowered = without_numbers.to_lowercase();
    let normalized = ws.replace_all(&lowered, " ");
    normalized.trim().to_string()
}

/// Produce a stable fingerprint (hex SHA-256) for a canonical query.
pub fn fingerprint(query: &str) -> String {
    let canon = canonicalize_query(query);
    let mut hasher = Sha256::new();
    hasher.update(canon.as_bytes());
    let res = hasher.finalize();
    hex::encode(res)
}
