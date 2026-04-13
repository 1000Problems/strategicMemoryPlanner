use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::config::OllamaConfig;
use super::Secretary;

/// Ollama HTTP backend. Calls localhost:11434 (or configured URL).
pub struct OllamaSecretary {
    client: reqwest::Client,
    url: String,
    model: String,
}

#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    system: String,
    stream: bool,
    format: serde_json::Value,
    options: OllamaOptions,
}

#[derive(Serialize)]
struct OllamaOptions {
    temperature: f64,
    top_p: f64,
    num_predict: i32,
}

#[derive(Deserialize)]
struct OllamaResponse {
    response: String,
}

impl OllamaSecretary {
    pub fn new(config: &OllamaConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: config.url.trim_end_matches('/').to_string(),
            model: config.model.clone(),
        }
    }
}

#[async_trait]
impl Secretary for OllamaSecretary {
    async fn extract(&self, prompt: &str, json_schema: &str) -> Result<String> {
        let request = OllamaRequest {
            model: self.model.clone(),
            prompt: prompt.to_string(),
            system: "You extract structured data from coding session transcripts. Output ONLY valid JSON.".to_string(),
            stream: false,
            format: serde_json::from_str(json_schema).unwrap_or(serde_json::json!("json")),
            options: OllamaOptions {
                temperature: 0.1,
                top_p: 0.9,
                num_predict: 2048,
            },
        };

        let response: OllamaResponse = self.client
            .post(format!("{}/api/generate", self.url))
            .json(&request)
            .send()
            .await
            .context("Failed to reach Ollama")?
            .json()
            .await
            .context("Failed to parse Ollama response")?;

        Ok(response.response.trim().to_string())
    }

    fn name(&self) -> &str {
        "ollama"
    }
}
