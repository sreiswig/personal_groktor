//! xAI Grok API client (chat completions compatible).

use serde::{Deserialize, Serialize};

use super::build_prompt;
use crate::error::{GroktorError, Result};
use crate::schema::{Finding, MetricPoint};

const DEFAULT_BASE: &str = "https://api.x.ai/v1";
const DEFAULT_MODEL: &str = "grok-3";

#[derive(Debug, Clone)]
pub struct GrokClient {
    api_key: String,
    base_url: String,
    model: String,
    http: reqwest::Client,
}

impl GrokClient {
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("XAI_API_KEY").map_err(|_| {
            GroktorError::Config(
                "XAI_API_KEY is not set. Export your xAI API key to enable Grok narratives."
                    .into(),
            )
        })?;
        let base_url = std::env::var("XAI_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE.into());
        let model = std::env::var("XAI_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
        Ok(Self::new(api_key, base_url, model))
    }

    pub fn new(api_key: impl Into<String>, base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            model: model.into(),
            http: reqwest::Client::new(),
        }
    }

    pub async fn narrate(
        &self,
        day_metrics: &[MetricPoint],
        findings: &[Finding],
    ) -> Result<String> {
        let prompt = build_prompt(day_metrics, findings);
        self.complete_raw(&prompt).await
    }

    /// Complete a free-form user prompt (system message is fixed non-clinical helper).
    pub async fn complete_raw(&self, user_prompt: &str) -> Result<String> {
        let body = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage {
                    role: "system".into(),
                    content: "You help people understand personal wearable data carefully and non-clinically.".into(),
                },
                ChatMessage {
                    role: "user".into(),
                    content: user_prompt.to_string(),
                },
            ],
            temperature: 0.4,
        };

        let url = format!("{}/chat/completions", self.base_url);
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(GroktorError::Llm(format!(
                "Grok API {status}: {}",
                truncate(&text, 400)
            )));
        }

        let parsed: ChatResponse = serde_json::from_str(&text).map_err(|e| {
            GroktorError::Llm(format!(
                "failed to parse Grok response: {e}; body={}",
                truncate(&text, 200)
            ))
        })?;

        parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| GroktorError::Llm("empty Grok response".into()))
    }
}

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ChatMessage,
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}
