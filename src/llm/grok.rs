//! OpenAI-compatible chat completions client (xAI Grok + local).

use serde::{Deserialize, Serialize};

use super::build_prompt;
use crate::error::{GroktorError, Result};
use crate::schema::{Finding, MetricPoint};

const DEFAULT_GROK_BASE: &str = "https://api.x.ai/v1";
const DEFAULT_GROK_MODEL: &str = "grok-3";
const DEFAULT_LOCAL_KEY: &str = "local";

/// Which LLM endpoint `brief --llm` / `digest --llm` should call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmBackend {
    Grok,
    Local,
}

impl LlmBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Grok => "grok",
            Self::Local => "local",
        }
    }

    /// Parse `grok` or `local` (case-insensitive). Used by tests and callers.
    pub fn parse_name(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "grok" => Ok(Self::Grok),
            "local" => Ok(Self::Local),
            other => Err(GroktorError::Parse(format!(
                "unknown LLM backend `{other}` (expected grok or local)"
            ))),
        }
    }

    fn api_label(self) -> &'static str {
        match self {
            Self::Grok => "Grok API",
            Self::Local => "local LLM API",
        }
    }
}

/// Resolved chat-completions settings (no HTTP).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
}

fn nonempty(v: Option<String>) -> Option<String> {
    v.and_then(|s| {
        let t = s.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    })
}

fn env_nonempty(key: &str) -> Option<String> {
    nonempty(std::env::var(key).ok())
}

/// Grok env: `XAI_API_KEY` required; `XAI_BASE_URL` / `XAI_MODEL` optional.
pub fn resolve_grok_config(
    api_key: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
) -> Result<ChatConfig> {
    let api_key = nonempty(api_key).ok_or_else(|| {
        GroktorError::Config(
            "XAI_API_KEY is not set. Export your xAI API key to enable Grok narratives.".into(),
        )
    })?;
    Ok(ChatConfig {
        api_key,
        base_url: nonempty(base_url).unwrap_or_else(|| DEFAULT_GROK_BASE.into()),
        model: nonempty(model).unwrap_or_else(|| DEFAULT_GROK_MODEL.into()),
    })
}

/// Local OpenAI-compatible env: base + model required; key defaults to `local`.
pub fn resolve_local_config(
    base_url: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
) -> Result<ChatConfig> {
    let base_url = nonempty(base_url).ok_or_else(|| {
        GroktorError::Config(
            "--llm-backend local requires GROKTOR_LLM_BASE or LLM_API_BASE \
             (no default; will not phone home)."
                .into(),
        )
    })?;
    let model = nonempty(model).ok_or_else(|| {
        GroktorError::Config("--llm-backend local requires GROKTOR_LLM_MODEL or LLM_MODEL.".into())
    })?;
    Ok(ChatConfig {
        api_key: nonempty(api_key).unwrap_or_else(|| DEFAULT_LOCAL_KEY.into()),
        base_url,
        model,
    })
}

/// Generic OpenAI-compatible chat client. `GrokClient` is a compatibility alias.
#[derive(Debug, Clone)]
pub struct ChatClient {
    api_key: String,
    base_url: String,
    model: String,
    backend: LlmBackend,
    http: reqwest::Client,
}

/// Existing name: same HTTP path as [`ChatClient`].
pub type GrokClient = ChatClient;

impl ChatClient {
    pub fn from_env() -> Result<Self> {
        Self::from_grok_env()
    }

    pub fn from_grok_env() -> Result<Self> {
        let cfg = resolve_grok_config(
            env_nonempty("XAI_API_KEY"),
            env_nonempty("XAI_BASE_URL"),
            env_nonempty("XAI_MODEL"),
        )?;
        Ok(Self::from_config(cfg, LlmBackend::Grok))
    }

    pub fn from_local_env() -> Result<Self> {
        let cfg = resolve_local_config(
            env_nonempty("GROKTOR_LLM_BASE").or_else(|| env_nonempty("LLM_API_BASE")),
            env_nonempty("GROKTOR_LLM_MODEL").or_else(|| env_nonempty("LLM_MODEL")),
            env_nonempty("GROKTOR_LLM_API_KEY").or_else(|| env_nonempty("LLM_API_KEY")),
        )?;
        Ok(Self::from_config(cfg, LlmBackend::Local))
    }

    pub fn from_backend(backend: LlmBackend) -> Result<Self> {
        match backend {
            LlmBackend::Grok => Self::from_grok_env(),
            LlmBackend::Local => Self::from_local_env(),
        }
    }

    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self::from_config(
            ChatConfig {
                api_key: api_key.into(),
                base_url: base_url.into(),
                model: model.into(),
            },
            LlmBackend::Grok,
        )
    }

    fn from_config(cfg: ChatConfig, backend: LlmBackend) -> Self {
        Self {
            api_key: cfg.api_key,
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            model: cfg.model,
            backend,
            http: reqwest::Client::new(),
        }
    }

    pub fn backend(&self) -> LlmBackend {
        self.backend
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
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
        let label = self.backend.api_label();
        if !status.is_success() {
            return Err(GroktorError::Llm(format!(
                "{label} {status}: {}",
                truncate(&text, 400)
            )));
        }

        let parsed: ChatResponse = serde_json::from_str(&text).map_err(|e| {
            GroktorError::Llm(format!(
                "failed to parse {label} response: {e}; body={}",
                truncate(&text, 200)
            ))
        })?;

        parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| GroktorError::Llm(format!("empty {label} response")))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_selection_grok_and_local() {
        assert_eq!(LlmBackend::parse_name("grok").unwrap(), LlmBackend::Grok);
        assert_eq!(LlmBackend::parse_name("GROK").unwrap(), LlmBackend::Grok);
        assert_eq!(
            LlmBackend::parse_name(" local ").unwrap(),
            LlmBackend::Local
        );
        assert_eq!(LlmBackend::Grok.as_str(), "grok");
        assert_eq!(LlmBackend::Local.as_str(), "local");
        let err = LlmBackend::parse_name("openai").unwrap_err();
        assert!(err.to_string().contains("unknown LLM backend"));
    }

    #[test]
    fn grok_requires_api_key_and_keeps_defaults() {
        let err = resolve_grok_config(None, None, None).unwrap_err();
        assert!(err.to_string().contains("XAI_API_KEY"));

        let cfg = resolve_grok_config(Some("sk-test".into()), None, None).unwrap();
        assert_eq!(cfg.api_key, "sk-test");
        assert_eq!(cfg.base_url, DEFAULT_GROK_BASE);
        assert_eq!(cfg.model, DEFAULT_GROK_MODEL);

        let cfg = resolve_grok_config(
            Some("sk-test".into()),
            Some("https://example.invalid/v1/".into()),
            Some("grok-custom".into()),
        )
        .unwrap();
        assert_eq!(cfg.base_url, "https://example.invalid/v1/");
        assert_eq!(cfg.model, "grok-custom");
    }

    #[test]
    fn local_requires_base_and_model_defaults_key() {
        let err = resolve_local_config(None, Some("mistral".into()), None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("GROKTOR_LLM_BASE") || msg.contains("LLM_API_BASE"));
        assert!(msg.contains("will not phone home"));

        let err =
            resolve_local_config(Some("http://127.0.0.1:8000/v1".into()), None, None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("GROKTOR_LLM_MODEL") || msg.contains("LLM_MODEL"));

        let err = resolve_local_config(Some("   ".into()), Some("m".into()), None).unwrap_err();
        assert!(err.to_string().contains("GROKTOR_LLM_BASE"));

        let cfg = resolve_local_config(
            Some("http://127.0.0.1:8000/v1".into()),
            Some("spark-model".into()),
            None,
        )
        .unwrap();
        assert_eq!(cfg.api_key, DEFAULT_LOCAL_KEY);
        assert_eq!(cfg.base_url, "http://127.0.0.1:8000/v1");
        assert_eq!(cfg.model, "spark-model");

        let cfg = resolve_local_config(
            Some("http://192.168.1.10:8000/v1".into()),
            Some("llama".into()),
            Some("secret".into()),
        )
        .unwrap();
        assert_eq!(cfg.api_key, "secret");
    }

    #[test]
    fn client_new_strips_trailing_slash_and_records_grok_backend() {
        let client = ChatClient::new("sk", "https://api.x.ai/v1/", "grok-3");
        assert_eq!(client.backend(), LlmBackend::Grok);
        assert_eq!(client.base_url(), "https://api.x.ai/v1");
    }
}
