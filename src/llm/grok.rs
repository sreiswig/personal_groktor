//! OpenAI-compatible chat completions client (xAI Grok + local).

use std::net::IpAddr;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::build_prompt;
use crate::error::{GroktorError, Result};
use crate::schema::{Finding, MetricPoint};

const DEFAULT_GROK_BASE: &str = "https://api.x.ai/v1";
const DEFAULT_GROK_MODEL: &str = "grok-3";
const DEFAULT_LOCAL_KEY: &str = "local";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

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
    let base_url = nonempty(base_url).unwrap_or_else(|| DEFAULT_GROK_BASE.into());
    ensure_grok_base_url(&base_url)?;
    Ok(ChatConfig {
        api_key,
        base_url,
        model: nonempty(model).unwrap_or_else(|| DEFAULT_GROK_MODEL.into()),
    })
}

/// Grok sends `XAI_API_KEY` as a bearer token. Require HTTPS unless the host is
/// loopback so that key never travels in the clear to a remote host.
fn ensure_grok_base_url(base_url: &str) -> Result<()> {
    if grok_base_url_allowed(base_url) {
        return Ok(());
    }
    Err(GroktorError::Config(format!(
        "Grok base URL must use https unless the host is loopback \
         (localhost, 127.0.0.0/8, or ::1); refusing `{base_url}` so the API key \
         is not sent over plaintext HTTP"
    )))
}

fn grok_base_url_allowed(base_url: &str) -> bool {
    let Some((scheme, rest)) = base_url.split_once("://") else {
        return false;
    };
    if scheme.eq_ignore_ascii_case("https") {
        return true;
    }
    scheme.eq_ignore_ascii_case("http") && http_host_is_loopback(rest)
}

fn http_host_is_loopback(after_scheme: &str) -> bool {
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return false;
    }
    let hostport = match authority.rsplit_once('@') {
        Some((_, hostport)) => hostport,
        None => authority,
    };
    let host = if let Some(rest) = hostport.strip_prefix('[') {
        let Some((host, after)) = rest.split_once(']') else {
            return false;
        };
        if !(after.is_empty() || after.starts_with(':')) {
            return false;
        }
        host
    } else if let Some((host, port)) = hostport.split_once(':') {
        if port.contains(':') {
            return false;
        }
        host
    } else {
        hostport
    };
    if host.is_empty() {
        return false;
    }
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    matches!(host.parse::<IpAddr>(), Ok(ip) if ip.is_loopback())
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
            http: Self::build_http_client(),
        }
    }

    fn build_http_client() -> reqwest::Client {
        reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("HTTP client with fixed connect and request timeouts")
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
        // `floor_char_boundary` never cuts inside a multibyte scalar, so a
        // response body cannot panic this helper (which would take down brief).
        let end = s.floor_char_boundary(max);
        format!("{}…", &s[..end])
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

    #[test]
    fn truncate_is_char_safe_for_ascii_short_and_multibyte_cuts() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("", 4), "");
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello world", 5), "hello…");

        // 你 is 3 bytes (E4 BD A0). Index 2 falls inside that scalar.
        let cjk = "a你b";
        assert_eq!(cjk.len(), 5);
        assert!(!cjk.is_char_boundary(2));
        assert_eq!(truncate(cjk, 2), "a…");
        assert_eq!(truncate(cjk, 4), "a你…");

        // 😀 is 4 bytes. Index 3 falls inside that scalar.
        let emoji = "go😀!";
        assert_eq!(emoji.len(), 7);
        assert!(!emoji.is_char_boundary(3));
        assert_eq!(truncate(emoji, 1), "g…");
        assert_eq!(truncate(emoji, 3), "go…");
        assert_eq!(truncate(emoji, 6), "go😀…");
        assert_eq!(truncate("😀", 2), "…");
    }

    #[test]
    fn grok_base_url_requires_https_unless_loopback() {
        let cfg = resolve_grok_config(Some("sk-test".into()), None, None).unwrap();
        assert_eq!(cfg.base_url, DEFAULT_GROK_BASE);
        assert_eq!(cfg.model, DEFAULT_GROK_MODEL);

        let cfg = resolve_grok_config(
            Some("sk-test".into()),
            Some("https://example.invalid/v1".into()),
            None,
        )
        .unwrap();
        assert_eq!(cfg.base_url, "https://example.invalid/v1");

        let cfg = resolve_grok_config(
            Some("sk-test".into()),
            Some("HTTPS://api.x.ai/v1".into()),
            None,
        )
        .unwrap();
        assert_eq!(cfg.base_url, "HTTPS://api.x.ai/v1");

        for loopback in [
            "http://localhost:8080/v1",
            "http://LOCALHOST/v1",
            "http://127.0.0.1:8080/v1",
            "http://127.1.2.3/v1",
            "http://[::1]:8080/v1",
            "http://[::1]/v1",
        ] {
            let cfg =
                resolve_grok_config(Some("sk-test".into()), Some(loopback.into()), None).unwrap();
            assert_eq!(cfg.base_url, loopback, "{loopback}");
        }

        for remote in [
            "http://api.example.com/v1",
            "http://192.168.1.10:8000/v1",
            "http://127.0.0.1.example.com/v1",
            "http://localhost.example.com/v1",
            "http://[::1]@evil.example/v1",
        ] {
            let err =
                resolve_grok_config(Some("sk-test".into()), Some(remote.into()), None).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("https"), "{remote}: {msg}");
            assert!(msg.contains("plaintext HTTP"), "{remote}: {msg}");
            assert!(msg.contains(remote), "{remote}: {msg}");
        }
    }

    #[test]
    fn local_backend_allows_plaintext_http_on_lan_tailscale_and_loopback() {
        for base in [
            "http://127.0.0.1:8000/v1",
            "http://[::1]:8000/v1",
            "http://192.168.1.20:8000/v1",
            "http://10.1.2.3:8000/v1",
            "http://100.64.1.5:8000/v1",
        ] {
            let cfg = resolve_local_config(Some(base.into()), Some("spark".into()), None).unwrap();
            assert_eq!(cfg.base_url, base);
            assert_eq!(cfg.api_key, DEFAULT_LOCAL_KEY);
            assert_eq!(cfg.model, "spark");
        }
    }
}
