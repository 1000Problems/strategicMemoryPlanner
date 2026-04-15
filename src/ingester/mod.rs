pub mod filter;
pub mod mermaid;
pub mod parser;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::config::IngesterConfig;

/// A cleaned, compressed representation of a session transcript.
/// This is what the Secretary operates on — never the raw logs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Digest {
    pub messages: Vec<DigestMessage>,
    /// Estimated token count of the digest (rough: chars / 4).
    pub token_estimate: usize,
    /// Token estimate of the raw input before cleaning.
    pub raw_token_estimate: usize,
    /// Compression ratio achieved (raw / digest).
    pub compression_ratio: f64,
}

impl Digest {
    /// Render the digest as plain text for feeding to the Secretary.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for msg in &self.messages {
            out.push_str(&format!("[{}] ", msg.role));
            out.push_str(&msg.content);
            if !msg.tool_name.is_empty() {
                out.push_str(&format!(" (tool: {})", msg.tool_name));
            }
            out.push('\n');
        }
        out
    }
}

/// A single meaningful message extracted from a session log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DigestMessage {
    pub role: String,          // "user", "assistant", "tool_use", "tool_result"
    pub content: String,       // The actual text content
    pub tool_name: String,     // For tool_use/tool_result: the tool name
    pub files_mentioned: Vec<String>,  // File paths extracted from content
}

/// Ingest a session log file and produce a cleaned Digest.
pub fn ingest(path: &Path, config: &IngesterConfig) -> Result<Digest> {
    let raw_content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read log file: {}", path.display()))?;

    let raw_token_estimate = raw_content.len() / 4;

    // Try parsing as JSONL first, fall back to plain text
    let raw_messages = if path.extension().map_or(false, |ext| ext == "jsonl") {
        parser::parse_jsonl(&raw_content)?
    } else if path.extension().map_or(false, |ext| ext == "json") {
        parser::parse_json_array(&raw_content)?
    } else {
        parser::parse_plain_text(&raw_content)
    };

    tracing::info!(
        raw_messages = raw_messages.len(),
        raw_tokens = raw_token_estimate,
        "Parsed raw messages"
    );

    // Filter and compress
    let messages = filter::filter_messages(raw_messages, config);
    let digest_text_len: usize = messages.iter().map(|m| m.content.len()).sum();
    let token_estimate = digest_text_len / 4;

    let compression_ratio = if token_estimate > 0 {
        raw_token_estimate as f64 / token_estimate as f64
    } else {
        1.0
    };

    tracing::info!(
        digest_messages = messages.len(),
        digest_tokens = token_estimate,
        compression = format!("{:.1}x", compression_ratio),
        "Digest ready"
    );

    Ok(Digest {
        messages,
        token_estimate,
        raw_token_estimate,
        compression_ratio,
    })
}
