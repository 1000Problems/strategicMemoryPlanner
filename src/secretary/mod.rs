pub mod embedded;
pub mod extract;
pub mod ollama;
pub mod openai;
pub mod prompts;

use anyhow::Result;
use async_trait::async_trait;

use crate::config::{Config, SecretaryBackend};

/// The Secretary trait — all LLM inference goes through this.
/// Backends: embedded (llama-cpp-2), ollama (HTTP), openai-compat (HTTP).
#[async_trait]
pub trait Secretary: Send + Sync {
    /// Run extraction. Sends `prompt` to the model and constrains output
    /// to valid JSON matching `json_schema` (used for GBNF grammar in
    /// embedded backend, or schema hints in HTTP backends).
    async fn extract(&self, prompt: &str, json_schema: &str) -> Result<String>;

    /// Human-readable name of this backend.
    fn name(&self) -> &str;
}

/// Build the appropriate Secretary backend from config.
pub fn build_secretary(config: &Config) -> Result<Box<dyn Secretary>> {
    match config.secretary.backend {
        SecretaryBackend::Embedded => {
            let embedded_config = config.secretary.embedded.as_ref()
                .ok_or_else(|| anyhow::anyhow!("secretary.embedded config required when backend = 'embedded'"))?;
            let secretary = embedded::EmbeddedSecretary::new(embedded_config)?;
            Ok(Box::new(secretary))
        }
        SecretaryBackend::Ollama => {
            let ollama_config = config.secretary.ollama.as_ref()
                .ok_or_else(|| anyhow::anyhow!("secretary.ollama config required when backend = 'ollama'"))?;
            let secretary = ollama::OllamaSecretary::new(ollama_config);
            Ok(Box::new(secretary))
        }
        SecretaryBackend::OpenaiCompat => {
            let openai_config = config.secretary.openai_compat.as_ref()
                .ok_or_else(|| anyhow::anyhow!("secretary.openai_compat config required when backend = 'openai-compat'"))?;
            let secretary = openai::OpenAICompatSecretary::new(openai_config);
            Ok(Box::new(secretary))
        }
    }
}
