use crate::config::{LLMRefineConfig, ProviderKeyConfig, STORED_SECRET_PLACEHOLDER};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const REFINE_SYSTEM_PROMPT: &str = "You are a careful speech-to-text editor. Fix punctuation, casing, and obvious ASR errors. Preserve the speaker's meaning and language. Return only the revised text.";

const TRANSLATE_SYSTEM_PROMPT: &str = "You are a precise translator. Translate the user's text into the target language. Preserve meaning, tone, and formatting. Return only the translation.";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefineConnectionResult {
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Option<Vec<ChatChoice>>,
    error: Option<ChatError>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: Option<ChatMessage>,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    content: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct ChatError {
    message: Option<String>,
}

fn resolve_connection(config: &LLMRefineConfig) -> (String, String, String) {
    let provider = config.provider.as_str();
    let (endpoint, model, key) = match provider {
        "openai" => (
            if config.endpoint.trim().is_empty() {
                "https://api.openai.com/v1".into()
            } else {
                config.endpoint.clone()
            },
            if config.openai.model.is_empty() {
                config.model.clone()
            } else {
                config.openai.model.clone()
            },
            pick_key(&config.openai.api_key, &config.api_key),
        ),
        "deepseek" => (
            if config.endpoint.trim().is_empty() {
                "https://api.deepseek.com".into()
            } else {
                config.endpoint.clone()
            },
            if config.deepseek.model.is_empty() {
                config.model.clone()
            } else {
                config.deepseek.model.clone()
            },
            pick_key(&config.deepseek.api_key, &config.api_key),
        ),
        "openrouter" => (
            if config.endpoint.trim().is_empty() {
                "https://openrouter.ai/api/v1".into()
            } else {
                config.endpoint.clone()
            },
            if config.openrouter.model.is_empty() {
                config.model.clone()
            } else {
                config.openrouter.model.clone()
            },
            pick_key(&config.openrouter.api_key, &config.api_key),
        ),
        "tokendance" => (
            if config.endpoint.trim().is_empty() {
                "https://tokendance.space/gateway/v1".into()
            } else {
                config.endpoint.clone()
            },
            if config.tokendance.model.is_empty() {
                config.model.clone()
            } else {
                config.tokendance.model.clone()
            },
            pick_key(&config.tokendance.api_key, &config.api_key),
        ),
        "mimo" => (
            if config.mimo.endpoint.trim().is_empty() {
                "https://api.xiaomimimo.com/v1".into()
            } else {
                config.mimo.endpoint.clone()
            },
            if config.mimo.model.trim().is_empty() {
                "mimo-v2.5".into()
            } else {
                config.mimo.model.clone()
            },
            pick_key(&config.mimo.api_key, &config.api_key),
        ),
        _ => {
            // custom-compatible and any future provider: prefer dedicated custom fields.
            let endpoint = if !config.custom.endpoint.trim().is_empty() {
                config.custom.endpoint.clone()
            } else {
                config.endpoint.clone()
            };
            let model = if !config.custom.model.trim().is_empty() {
                config.custom.model.clone()
            } else {
                config.model.clone()
            };
            let key = pick_key(&config.custom.api_key, &config.api_key);
            (endpoint, model, key)
        }
    };

    let base = endpoint.trim().trim_end_matches('/').to_string();
    let base = base
        .strip_suffix("/chat/completions")
        .unwrap_or(&base)
        .trim_end_matches('/')
        .to_string();
    let url = format!("{base}/chat/completions");
    (url, model, key)
}

fn pick_key(provider_key: &str, fallback: &str) -> String {
    let value = if provider_key.trim().is_empty() {
        fallback
    } else {
        provider_key
    };
    if value == STORED_SECRET_PLACEHOLDER {
        String::new()
    } else {
        value.to_string()
    }
}

fn require_https_or_localhost(url: &str) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|e| format!("Invalid endpoint: {e}"))?;
    let host = parsed.host_str().unwrap_or("");
    let is_local = host == "localhost" || host == "127.0.0.1" || host == "::1";
    if parsed.scheme() == "https" || (parsed.scheme() == "http" && is_local) {
        Ok(())
    } else {
        Err("Custom endpoint must use https (http allowed only for localhost)".into())
    }
}

async fn chat_completion(
    system: &str,
    user: &str,
    config: &LLMRefineConfig,
) -> Result<String, String> {
    let (url, model, api_key) = resolve_connection(config);
    if url.is_empty() || model.is_empty() || api_key.is_empty() {
        return Err("LLM connection is incomplete".into());
    }
    require_https_or_localhost(&url)?;

    let payload = json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
        "temperature": 0.2,
        // Bound completion length adaptively. A rewritten / translated copy of
        // the input needs roughly one token per input char (CJK) and fewer for
        // Latin scripts, so `input + margin` never truncates normal output;
        // the floor leaves short push-to-talk utterances room for reasoning,
        // and the ceiling stops a thinking model from wandering.
        "max_tokens": (user.chars().count() + 256).clamp(512, 8192),
    });

    let response = crate::state::shared_http_client()
        .post(&url)
        .timeout(std::time::Duration::from_secs(60))
        .bearer_auth(api_key)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("LLM request failed: {e}"))?;

    let status = response.status();
    let body: ChatResponse = response.json().await.map_err(|e| e.to_string())?;

    if !status.is_success() {
        let message = body
            .error
            .and_then(|e| e.message)
            .unwrap_or_else(|| format!("LLM returned HTTP {status}"));
        return Err(message);
    }

    let content = body
        .choices
        .and_then(|c| c.into_iter().next())
        .and_then(|c| c.message)
        .and_then(|m| m.content)
        .map(|c| match c {
            Value::String(s) => s,
            other => other.to_string(),
        })
        .unwrap_or_default()
        .trim()
        .to_string();

    Ok(strip_think(&content))
}

fn strip_think(content: &str) -> String {
    let trimmed = content.trim();
    if let Some(rest) = trimmed.strip_prefix("<think>") {
        return match rest.find("</think>") {
            Some(end) => rest[end + "</think>".len()..].trim().to_string(),
            // Unterminated think block (e.g. max_tokens cut the output short):
            // there is no usable answer, and callers keep the raw text on an
            // empty result instead of injecting think garbage.
            None => String::new(),
        };
    }
    trimmed.to_string()
}

pub async fn refine_text(text: &str, config: &LLMRefineConfig) -> Result<String, String> {
    chat_completion(REFINE_SYSTEM_PROMPT, text, config).await
}

pub async fn translate_text(
    text: &str,
    target_language: &str,
    config: &LLMRefineConfig,
) -> Result<String, String> {
    let user = format!("Target language: {target_language}\n\n{text}");
    chat_completion(TRANSLATE_SYSTEM_PROMPT, &user, config).await
}

/// Whether the LLM connection resolves to a complete endpoint / model / API
/// key. Clipboard-touching flows (selection translation) check this *before*
/// any side effects so an unconfigured connection fails fast with a visible
/// hint — the role of the Electron blueprint's `resolveConfig` pre-check.
pub fn connection_ready(config: &LLMRefineConfig) -> bool {
    let (url, model, api_key) = resolve_connection(config);
    !url.is_empty() && !model.is_empty() && !api_key.is_empty()
}

pub async fn test_connection(config: &LLMRefineConfig) -> RefineConnectionResult {
    match chat_completion("Reply with the single word: ok", "ping", config).await {
        Ok(_) => RefineConnectionResult {
            ok: true,
            message: "Connection succeeded".into(),
        },
        Err(message) => RefineConnectionResult { ok: false, message },
    }
}

pub fn normalize_provider_config(_raw: &ProviderKeyConfig) -> ProviderKeyConfig {
    _raw.clone()
}
