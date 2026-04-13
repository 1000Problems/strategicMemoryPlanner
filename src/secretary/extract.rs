use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::prompts::{schemas, PromptLoader};
use super::Secretary;
use crate::ingester::Digest;

/// A decision extracted from a session transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedDecision {
    pub decision: String,
    pub rationale: String,
    pub domain: String,
    #[serde(default)]
    pub alternatives_rejected: Vec<String>,
    #[serde(default)]
    pub files: Vec<String>,
}

/// A detected phase transition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedPhase {
    pub domain: String,
    pub phase: String,
    pub confidence: f64,
    pub signal: String,
}

/// Result of a full extraction run on a digest.
#[derive(Debug, Clone, Serialize)]
pub struct ExtractionResult {
    pub decisions: Vec<ExtractedDecision>,
    pub phase: Option<ExtractedPhase>,
    pub digest_tokens: usize,
}

/// Run decision extraction on a digest.
pub async fn extract_decisions(
    secretary: &dyn Secretary,
    prompts: &PromptLoader,
    digest: &Digest,
) -> Result<Vec<ExtractedDecision>> {
    let transcript_text = digest.to_text();

    // Chunk if needed — for now, treat as single chunk.
    // TODO: Add chunking for transcripts > model context size.
    let chunks = chunk_text(&transcript_text, 6000); // ~6k tokens safe for 8k context

    let mut all_decisions = Vec::new();

    for chunk in &chunks {
        let prompt = prompts.load("extract_decisions", chunk)
            .context("Failed to load decision extraction prompt")?;

        let raw_output = secretary.extract(&prompt, schemas::DECISIONS).await
            .context("Secretary failed during decision extraction")?;

        match serde_json::from_str::<Vec<ExtractedDecision>>(&raw_output) {
            Ok(decisions) => {
                tracing::debug!(count = decisions.len(), "Extracted decisions from chunk");
                all_decisions.extend(decisions);
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    raw = %raw_output.chars().take(200).collect::<String>(),
                    "Failed to parse decision extraction output, skipping chunk"
                );
            }
        }
    }

    Ok(all_decisions)
}

/// Run phase detection on a digest.
pub async fn detect_phase(
    secretary: &dyn Secretary,
    prompts: &PromptLoader,
    digest: &Digest,
) -> Result<Option<ExtractedPhase>> {
    // For phase detection, use the last portion of the transcript
    // (phase is most evident at the end of a session)
    let text = digest.to_text();
    let tail = tail_text(&text, 3000); // Last ~3k tokens

    let prompt = prompts.load("detect_phase", &tail)
        .context("Failed to load phase detection prompt")?;

    let raw_output = secretary.extract(&prompt, schemas::PHASE).await
        .context("Secretary failed during phase detection")?;

    match serde_json::from_str::<ExtractedPhase>(&raw_output) {
        Ok(phase) => {
            if phase.confidence >= 0.7 {
                Ok(Some(phase))
            } else {
                tracing::debug!(confidence = phase.confidence, "Phase detection below threshold");
                Ok(None)
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to parse phase detection output");
            Ok(None)
        }
    }
}

/// Split text into chunks of approximately `max_tokens` size.
/// Simple word-boundary splitting — good enough for extraction.
fn chunk_text(text: &str, max_tokens: usize) -> Vec<String> {
    // Rough heuristic: 1 token ≈ 4 chars for English/code
    let max_chars = max_tokens * 4;

    if text.len() <= max_chars {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;

    while start < text.len() {
        let end = (start + max_chars).min(text.len());

        // Find a good break point (newline or sentence boundary)
        let break_at = if end >= text.len() {
            end
        } else {
            text[start..end]
                .rfind('\n')
                .map(|pos| start + pos + 1)
                .unwrap_or(end)
        };

        chunks.push(text[start..break_at].to_string());
        start = break_at;
    }

    chunks
}

/// Get the last ~max_tokens worth of text.
fn tail_text(text: &str, max_tokens: usize) -> String {
    let max_chars = max_tokens * 4;
    if text.len() <= max_chars {
        return text.to_string();
    }
    let start = text.len() - max_chars;
    // Find a clean line break
    let clean_start = text[start..].find('\n').map(|p| start + p + 1).unwrap_or(start);
    text[clean_start..].to_string()
}
