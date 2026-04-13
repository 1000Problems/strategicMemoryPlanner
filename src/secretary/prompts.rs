use anyhow::{Context, Result};
use std::path::Path;

/// Loads prompt templates from disk and interpolates variables.
pub struct PromptLoader {
    prompts_dir: std::path::PathBuf,
}

impl PromptLoader {
    pub fn new(prompts_dir: &Path) -> Self {
        Self {
            prompts_dir: prompts_dir.to_path_buf(),
        }
    }

    /// Load a prompt template and replace {chunk} with the transcript text.
    pub fn load(&self, name: &str, chunk: &str) -> Result<String> {
        let path = self.prompts_dir.join(format!("{}.txt", name));
        let template = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to load prompt: {}", path.display()))?;
        Ok(template.replace("{chunk}", chunk))
    }
}

/// JSON schemas for constraining LLM output.
/// Used by embedded backend for GBNF grammar, and by HTTP backends as hints.
pub mod schemas {
    pub const DECISIONS: &str = r#"{
        "type": "array",
        "items": {
            "type": "object",
            "required": ["decision", "rationale", "domain"],
            "properties": {
                "decision": { "type": "string" },
                "rationale": { "type": "string" },
                "domain": { "type": "string" },
                "alternatives_rejected": {
                    "type": "array",
                    "items": { "type": "string" }
                },
                "files": {
                    "type": "array",
                    "items": { "type": "string" }
                }
            }
        }
    }"#;

    pub const PHASE: &str = r#"{
        "type": "object",
        "required": ["domain", "phase", "confidence", "signal"],
        "properties": {
            "domain": { "type": "string" },
            "phase": {
                "type": "string",
                "enum": ["exploring", "design", "ready", "blocked", "review", "done"]
            },
            "confidence": { "type": "number" },
            "signal": { "type": "string" }
        }
    }"#;
}
