use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::config::OpenAICompatConfig;
use super::Secretary;

/// OpenAI-compatible HTTP backend. Works with LM Studio, llama-server, vLLM, etc.
pub struct OpenAICompatSecretary {
    client: reqwest::Client,
    url: String,
    model: String,
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f64,
    max_tokens: u32,
    response_format: ResponseFormat,
}

#[derive(Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct ResponseFormat {
    r#type: String,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    content: String,
}

impl OpenAICompatSecretary {
    pub fn new(config: &OpenAICompatConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            url: config.url.trim_end_matches('/').to_string(),
            model: config.model.clone(),
        }
    }
}

#[async_trait]
impl Secretary for OpenAICompatSecretary {
    async fn extract(&self, prompt: &str, _json_schema: &str) -> Result<String> {
        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage {
                    role: "system".into(),
                    content: "You extract structured data from coding session transcripts. Output ONLY valid JSON.".into(),
                },
                ChatMessage {
                    role: "user".into(),
                    content: prompt.to_string(),
                },
            ],
            temperature: 0.1,
            max_tokens: 2048,
            response_format: ResponseFormat {
                r#type: "json_object".into(),
            },
        };

        let response: ChatResponse = self.client
            .post(format!("{}/chat/completions", self.url))
            .json(&request)
            .send()
            .await
            .context("Failed to reach OpenAI-compat server")?
            .json()
            .await
            .context("Failed to parse response")?;

        let content = response.choices
            .first()
            .map(|c| c.message.content.clone())
            .unwrap_or_default();

        Ok(content.trim().to_string())
    }

    fn name(&self) -> &str {
        "openai-compat"
    }
}
