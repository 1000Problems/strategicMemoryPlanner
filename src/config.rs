use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub server: ServerConfig,
    pub secretary: SecretaryConfig,
    pub ingester: IngesterConfig,
    pub extraction: ExtractionConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    pub port: u16,
    pub data_dir: PathBuf,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SecretaryConfig {
    pub backend: SecretaryBackend,
    pub prompts_dir: PathBuf,
    #[serde(default)]
    pub embedded: Option<EmbeddedConfig>,
    #[serde(default)]
    pub ollama: Option<OllamaConfig>,
    #[serde(default)]
    pub openai_compat: Option<OpenAICompatConfig>,
}

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SecretaryBackend {
    Embedded,
    Ollama,
    OpenaiCompat,
}

#[derive(Debug, Deserialize, Clone)]
pub struct EmbeddedConfig {
    pub model_path: PathBuf,
    #[serde(default = "default_gpu_layers")]
    pub gpu_layers: u32,
    #[serde(default = "default_context_size")]
    pub context_size: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct OllamaConfig {
    pub url: String,
    pub model: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct OpenAICompatConfig {
    pub url: String,
    pub model: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct IngesterConfig {
    #[serde(default = "default_max_tool_result_tokens")]
    pub max_tool_result_tokens: usize,
    #[serde(default = "default_true")]
    pub collapse_repeated_edits: bool,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ExtractionConfig {
    #[serde(default = "default_phase_confidence")]
    pub phase_confidence_threshold: f64,
}

fn default_gpu_layers() -> u32 { 99 }
fn default_context_size() -> u32 { 8192 }
fn default_max_tool_result_tokens() -> usize { 500 }
fn default_true() -> bool { true }
fn default_phase_confidence() -> f64 { 0.8 }

fn expand_tilde(path: &Path) -> PathBuf {
    let path_str = path.to_string_lossy();
    if path_str.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home).join(&path_str[2..])
        } else {
            path.to_path_buf()
        }
    } else {
        path.to_path_buf()
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config: {}", path.display()))?;
        let mut config: Config = toml::from_str(&content)
            .with_context(|| "Failed to parse config TOML")?;

        // Expand tilde in model_path if using embedded backend
        if let Some(ref mut embedded) = config.secretary.embedded {
            embedded.model_path = expand_tilde(&embedded.model_path);
        }

        Ok(config)
    }

    /// Resolve the data directory for a specific project.
    pub fn project_data_dir(&self, project: &str) -> PathBuf {
        self.server.data_dir.join(project)
    }

    /// Get the SQLite DB path for a project.
    pub fn project_db_path(&self, project: &str) -> PathBuf {
        self.project_data_dir(project).join("state.db")
    }
}
