use regex::Regex;
use sha2::{Digest as Sha2Digest, Sha256};
use std::fmt::Write;

/// A mermaid diagram extracted from a session transcript.
#[derive(Debug, Clone)]
pub struct ExtractedDiagram {
    pub title: Option<String>,
    pub diagram_type: String,
    pub content: String,    // raw mermaid text, no fences
    pub fingerprint: String, // sha256 hex of trimmed content
}

/// Extract all mermaid code blocks from raw session text.
/// Runs on the original file content before any filtering — works on JSONL
/// text so it searches for the fenced blocks embedded in JSON string values.
pub fn extract_mermaid(raw_text: &str) -> Vec<ExtractedDiagram> {
    // JSONL files store all content on a single line with \n as the two-char
    // escape sequence. Plain text/markdown uses actual newlines.
    // (?:\\n|\n) matches either: literal \n escape sequence OR actual newline.
    let re = Regex::new(r"```mermaid(?:\\n|\n)([\s\S]*?)(?:\\n|\n)```").unwrap();

    let mut results = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for cap in re.captures_iter(raw_text) {
        let raw_content = cap[1].to_string();
        // Unescape JSON \n sequences → actual newlines, then trim
        let content = raw_content
            .replace("\\n", "\n")
            .replace("\\t", "\t")
            .trim()
            .to_string();

        if content.is_empty() {
            continue;
        }

        let fingerprint = sha256_hex(&content);
        if seen.contains(&fingerprint) {
            continue;
        }
        seen.insert(fingerprint.clone());

        let diagram_type = extract_diagram_type(&content);
        let title = extract_title(raw_text, cap.get(0).map(|m| m.start()).unwrap_or(0));

        results.push(ExtractedDiagram {
            title,
            diagram_type,
            content,
            fingerprint,
        });
    }

    results
}

fn sha256_hex(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let result = hasher.finalize();
    let mut hex = String::with_capacity(64);
    for byte in result.iter() {
        write!(&mut hex, "{:02x}", byte).unwrap();
    }
    hex
}

/// The diagram type is the first word of the mermaid block.
fn extract_diagram_type(content: &str) -> String {
    content
        .lines()
        .find(|l| !l.trim().is_empty() && !l.trim().starts_with("%%"))
        .and_then(|l| l.split_whitespace().next())
        .unwrap_or("unknown")
        .to_lowercase()
        .to_string()
}

/// Scan backwards from the start of the mermaid fence to find a title.
/// Takes the last non-empty line before the block that ends with `:` or `-`.
fn extract_title(raw_text: &str, fence_start: usize) -> Option<String> {
    let before = raw_text.get(..fence_start).unwrap_or("");
    // Work backwards through lines, skip blanks, take first meaningful one
    let candidate = before
        .lines()
        .rev()
        .map(|l| l.trim())
        .find(|l| !l.is_empty() && l.len() > 4)?;

    // Strip trailing punctuation and JSON artifacts
    let cleaned = candidate
        .trim_end_matches([':',  '-', '"', '\\', 'n'])
        .trim()
        .to_string();

    if cleaned.len() < 4 || cleaned.len() > 120 {
        return None;
    }

    // Skip lines that look like JSON structure or code rather than prose
    let looks_like_json = cleaned.starts_with('{')
        || cleaned.starts_with('[')
        || cleaned.starts_with('"')
        || cleaned.ends_with('{')
        || cleaned.ends_with(',');

    if looks_like_json {
        return None;
    }

    Some(cleaned)
}
