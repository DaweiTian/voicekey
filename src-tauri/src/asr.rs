//! Generic cloud speech recognition (ASR), plus the shared WAV encoder.
//!
//! The request shape is chosen by [`CloudDialect`] so the app can talk to any
//! OpenAI-compatible cloud ASR: MiMo and Qwen (`chat_asr_data_url`), OpenAI
//! audio-chat models and Doubao Ark (`chat_openai_audio`), or OpenAI's
//! `/audio/transcriptions` multipart endpoint (`openai_transcribe`). Local ONNX
//! inference was removed in 0.5.11 — cloud is the only engine.
use base64::Engine;
use serde::{Deserialize, Serialize};

/// Wire format for a cloud ASR request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudDialect {
    /// `input_audio.data` carries a `data:` URL and the language rides a
    /// top-level `asr_options` object. MiMo (`mimo-v2.5-asr`) and Qwen
    /// (`qwen3-asr-flash`, DashScope compatible mode).
    ChatAsrDataUrl,
    /// `input_audio.data` carries raw base64 with an explicit `format`, plus a
    /// leading transcription-instruction text part. OpenAI audio-chat models
    /// and Doubao Ark audio-understanding models.
    ChatOpenaiAudio,
    /// Multipart `POST {base}/audio/transcriptions` (non-streaming). OpenAI and
    /// OpenAI-compatible gateways that implement the standard STT endpoint.
    OpenaiTranscribe,
}

impl CloudDialect {
    /// Parse the persisted `cloud_dialect` string; unknown values fall back to
    /// the MiMo/Qwen default so a corrupt config can never break recognition.
    pub fn from_config(value: &str) -> Self {
        match value {
            "chat_openai_audio" => Self::ChatOpenaiAudio,
            "openai_transcribe" => Self::OpenaiTranscribe,
            _ => Self::ChatAsrDataUrl,
        }
    }
}

/// Transcription instruction for the `chat_openai_audio` dialect. Ark's audio
/// understanding models need an explicit "transcribe only, no commentary"
/// prompt; OpenAI works with the standard English instruction. Selected by the
/// provider preset id.
pub fn chat_audio_instruction(preset: &str) -> &'static str {
    if preset == "doubao_ark" {
        "请识别音频中的内容，仅以文字形式返回识别结果，不要添加解释。"
    } else {
        "Transcribe this audio."
    }
}

/// Everything a cloud request needs. `instruction` is only consumed by the
/// `chat_openai_audio` dialect; other dialects ignore it.
#[derive(Debug, Clone, Copy)]
pub struct CloudCall<'a> {
    pub api_key: &'a str,
    pub base_url: &'a str,
    pub model: &'a str,
    pub language: &'a str,
    pub dialect: CloudDialect,
    pub instruction: &'a str,
}

/// Encode mono f32 samples (-1.0..1.0) as 16-bit PCM WAV.
pub fn encode_wav_mono_f32(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let data_len = samples.len() * 2;
    let mut wav = Vec::with_capacity(44 + data_len);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&((36 + data_len) as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
    wav.extend_from_slice(&2u16.to_le_bytes()); // block align
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data_len as u32).to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        wav.extend_from_slice(&v.to_le_bytes());
    }
    wav
}

/// Normalize a configured endpoint to `{base}/chat/completions`.
fn chat_completions_url(base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/').to_string();
    let base = base
        .strip_suffix("/chat/completions")
        .unwrap_or(&base)
        .trim_end_matches('/')
        .to_string();
    format!("{base}/chat/completions")
}

/// Normalize a configured endpoint to `{base}/audio/transcriptions`.
fn transcriptions_url(base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/').to_string();
    let base = base
        .strip_suffix("/audio/transcriptions")
        .unwrap_or(&base)
        .trim_end_matches('/')
        .to_string();
    format!("{base}/audio/transcriptions")
}

/// Smallest size guard shared by every dialect (MiMo 7MB / Qwen 10MB /
/// OpenAI 25MB / Ark 25MB — 7MB is the safe common denominator).
fn check_audio_len(audio_wav: &[u8]) -> Result<(), String> {
    if audio_wav.len() < 44 {
        return Err("录音为空，请重试。".into());
    }
    if audio_wav.len() > 7 * 1024 * 1024 {
        return Err("录音过长，云端识别上限约 7MB，请缩短后重试。".into());
    }
    Ok(())
}

/// Raw base64 of the captured WAV (no `data:` prefix).
fn audio_base64(audio_wav: &[u8]) -> Result<String, String> {
    check_audio_len(audio_wav)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(audio_wav))
}

/// Inline `data:` URL of the captured WAV.
fn audio_data_url(audio_wav: &[u8]) -> Result<String, String> {
    Ok(format!(
        "data:audio/wav;base64,{}",
        audio_base64(audio_wav)?
    ))
}

/// `chat_asr_data_url` body (MiMo / Qwen): a `data:` URL inside
/// `input_audio.data`, language in a top-level `asr_options`. Byte-for-byte
/// identical to the pre-0.5.12 payload.
fn asr_payload_data_url(model: &str, data_url: &str, language: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "messages": [{
            "role": "user",
            "content": [{
                "type": "input_audio",
                "input_audio": { "data": data_url }
            }]
        }],
        "asr_options": { "language": language }
    })
}

/// `chat_openai_audio` body (OpenAI audio chat / Doubao Ark): raw base64 with
/// an explicit `format`, plus a leading transcription-instruction text part.
fn asr_payload_openai_audio(model: &str, raw_b64: &str, instruction: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": instruction },
                {
                    "type": "input_audio",
                    "input_audio": { "data": raw_b64, "format": "wav" }
                }
            ]
        }]
    })
}

fn missing_key_error() -> String {
    "云端 API Key 为空，请到「设置 - 云端识别」填写。".into()
}

/// Cloud ASR, non-streaming. Chat dialects POST `{base}/chat/completions`;
/// `openai_transcribe` POSTs multipart to `{base}/audio/transcriptions`.
/// Returns the recognized transcript.
pub async fn transcribe_cloud(call: &CloudCall<'_>, audio_wav: &[u8]) -> Result<String, String> {
    if call.api_key.trim().is_empty() {
        return Err(missing_key_error());
    }
    let payload = match call.dialect {
        CloudDialect::OpenaiTranscribe => {
            return transcribe_openai_transcribe(call, audio_wav).await;
        }
        CloudDialect::ChatAsrDataUrl => {
            asr_payload_data_url(call.model, &audio_data_url(audio_wav)?, call.language)
        }
        CloudDialect::ChatOpenaiAudio => {
            asr_payload_openai_audio(call.model, &audio_base64(audio_wav)?, call.instruction)
        }
    };

    let url = chat_completions_url(call.base_url);
    let response = crate::state::shared_http_client()
        .post(&url)
        .timeout(std::time::Duration::from_secs(120))
        .bearer_auth(call.api_key)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("云端识别请求失败：{e}"))?;

    let status = response.status();
    let body: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;

    if !status.is_success() {
        let message = body
            .pointer("/error/message")
            .and_then(|v| v.as_str())
            .unwrap_or("ASR request failed")
            .to_string();
        return Err(format!("{message}（HTTP {status}）"));
    }

    let text = body
        .pointer("/choices/0/message/content")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    if text.is_empty() {
        return Err("云端识别未返回文字，请重试。".into());
    }
    Ok(text)
}

/// `openai_transcribe`: multipart `POST {base}/audio/transcriptions`. Always
/// non-streaming (the endpoint's `stream=true` event shape is not the chat
/// delta stream). `reqwest` sets the multipart `Content-Type` boundary.
async fn transcribe_openai_transcribe(
    call: &CloudCall<'_>,
    audio_wav: &[u8],
) -> Result<String, String> {
    check_audio_len(audio_wav)?;
    let url = transcriptions_url(call.base_url);

    let file = reqwest::multipart::Part::bytes(audio_wav.to_vec())
        .file_name("recording.wav")
        .mime_str("audio/wav")
        .map_err(|e| e.to_string())?;
    let mut form = reqwest::multipart::Form::new()
        .part("file", file)
        .text("model", call.model.to_string())
        .text("response_format", "json".to_string());
    // `auto` is the app's internal "detect" sentinel, not a valid ISO code;
    // omit the field so the endpoint auto-detects.
    if !call.language.trim().is_empty() && call.language != "auto" {
        form = form.text("language", call.language.to_string());
    }

    let response = crate::state::shared_http_client()
        .post(&url)
        .timeout(std::time::Duration::from_secs(120))
        .bearer_auth(call.api_key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("云端识别请求失败：{e}"))?;

    let status = response.status();
    // Error bodies may not be JSON; read the raw text first.
    let raw = response.text().await.unwrap_or_default();
    if !status.is_success() {
        let message = serde_json::from_str::<serde_json::Value>(&raw)
            .ok()
            .and_then(|v| {
                v.pointer("/error/message")
                    .and_then(|m| m.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| raw.chars().take(300).collect());
        return Err(format!("{message}（HTTP {status}）"));
    }

    let body: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("云端识别响应解析失败：{e}"))?;
    let text = body
        .pointer("/text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if text.is_empty() {
        return Err("云端识别未返回文字，请重试。".into());
    }
    Ok(text)
}

/// Parse one raw SSE line. Appends `choices[].delta.content` to `transcript`
/// and reports whether new text arrived; `done` flips on `data: [DONE]`.
/// `reasoning_content` deltas are deliberately ignored. Returns `Err` on a
/// malformed `data:` payload so the caller can fall back to non-streaming.
fn ingest_sse_line(line: &[u8], transcript: &mut String, done: &mut bool) -> Result<bool, String> {
    let line = String::from_utf8_lossy(line);
    let line = line.trim_end_matches(['\r', '\n']);
    if line.is_empty() || line.starts_with(':') {
        return Ok(false); // SSE comment / keep-alive
    }
    let Some(data) = line.strip_prefix("data:") else {
        return Ok(false); // `event:` / `id:` / `retry:` fields
    };
    let data = data.trim_start();
    if data == "[DONE]" {
        *done = true;
        return Ok(false);
    }
    let value: serde_json::Value =
        serde_json::from_str(data).map_err(|e| format!("云端识别流解析失败：{e}"))?;
    if let Some(message) = value.pointer("/error/message").and_then(|v| v.as_str()) {
        return Err(format!("{message}（SSE error）"));
    }
    let delta = value
        .pointer("/choices/0/delta/content")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if delta.is_empty() {
        return Ok(false);
    }
    transcript.push_str(delta);
    Ok(true)
}

/// Cloud ASR with SSE streaming (`"stream": true`) for the chat dialects.
///
/// `on_partial` receives the cumulative transcript so far every time the server
/// emits a `choices[].delta.content` increment, so the caller can surface live
/// progress (HUD) while the model is still generating. `openai_transcribe` has
/// no chat delta stream, so it is delegated to the non-streaming path. The
/// non-streaming `transcribe_cloud` remains the fallback: this function returns
/// `Err` on transport failures, HTTP errors, malformed SSE or empty output so
/// callers can retry once without streaming.
pub async fn transcribe_cloud_stream(
    call: &CloudCall<'_>,
    audio_wav: &[u8],
    mut on_partial: impl FnMut(&str),
) -> Result<String, String> {
    if call.api_key.trim().is_empty() {
        return Err(missing_key_error());
    }
    let mut payload = match call.dialect {
        CloudDialect::OpenaiTranscribe => {
            return transcribe_openai_transcribe(call, audio_wav).await;
        }
        CloudDialect::ChatAsrDataUrl => {
            asr_payload_data_url(call.model, &audio_data_url(audio_wav)?, call.language)
        }
        CloudDialect::ChatOpenaiAudio => {
            asr_payload_openai_audio(call.model, &audio_base64(audio_wav)?, call.instruction)
        }
    };
    payload["stream"] = serde_json::Value::Bool(true);
    let url = chat_completions_url(call.base_url);

    let response = crate::state::shared_http_client()
        .post(&url)
        .timeout(std::time::Duration::from_secs(120))
        .bearer_auth(call.api_key)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("云端识别请求失败：{e}"))?;

    let status = response.status();
    if !status.is_success() {
        // Error bodies may be JSON or SSE; read raw text for the message.
        let body = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| {
                v.pointer("/error/message")
                    .and_then(|m| m.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| body.chars().take(300).collect());
        return Err(format!("{message}（HTTP {status}）"));
    }

    // If the endpoint ignored `stream: true` it answers with a plain JSON
    // body; parse it directly so we never pay for a second audio upload.
    let is_sse = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.to_ascii_lowercase().contains("text/event-stream"))
        .unwrap_or(false);
    if !is_sse {
        let body: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
        let text = body
            .pointer("/choices/0/message/content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if text.is_empty() {
            return Err("云端识别未返回文字，请重试。".into());
        }
        return Ok(text);
    }

    // Incremental SSE parsing. Network chunks split arbitrarily (mid-line and
    // even mid-UTF-8 sequence), so buffer bytes and only decode whole lines.
    use futures_util::StreamExt;
    let mut stream = Box::pin(response.bytes_stream());
    let mut pending: Vec<u8> = Vec::new();
    let mut transcript = String::new();
    let mut done = false;

    'outer: while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("云端识别流中断：{e}"))?;
        pending.extend_from_slice(&chunk);
        while let Some(pos) = pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = pending.drain(..=pos).collect();
            if ingest_sse_line(&line, &mut transcript, &mut done)? {
                on_partial(transcript.as_str());
            }
            if done {
                break 'outer;
            }
        }
    }
    // A trailing line without its newline still counts (truncated streams).
    if !done && !pending.is_empty() {
        ingest_sse_line(&pending, &mut transcript, &mut done)?;
    }

    let text = transcript.trim().to_string();
    if text.is_empty() {
        return Err("云端识别未返回文字，请重试。".into());
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call<'a>(model: &'a str, dialect: CloudDialect, instruction: &'a str) -> CloudCall<'a> {
        CloudCall {
            api_key: "test-key",
            base_url: "https://example.com/v1",
            model,
            language: "auto",
            dialect,
            instruction,
        }
    }

    /// The exact 0.5.11 payload builder, frozen here as the byte-compatibility
    /// oracle for the MiMo/Qwen dialect.
    fn legacy_payload(model: &str, data_url: &str, language: &str) -> serde_json::Value {
        serde_json::json!({
            "model": model,
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "input_audio",
                    "input_audio": { "data": data_url }
                }]
            }],
            "asr_options": { "language": language }
        })
    }

    #[test]
    fn data_url_payload_is_byte_identical_to_legacy() {
        let model = "mimo-v2.5-asr";
        let data_url = "data:audio/wav;base64,QUJD";
        let new = asr_payload_data_url(model, data_url, "auto");
        let old = legacy_payload(model, data_url, "auto");
        assert_eq!(new, old, "structure drifted from the 0.5.11 payload");
        assert_eq!(
            serde_json::to_string(&new).unwrap(),
            serde_json::to_string(&old).unwrap(),
            "serialized bytes drifted from the 0.5.11 payload"
        );
        // No `format` field and no text part in this dialect.
        assert!(new
            .pointer("/messages/0/content/0/input_audio/format")
            .is_none());
        assert_eq!(new.pointer("/messages/0/content/1"), None);
    }

    #[test]
    fn openai_audio_payload_has_raw_base64_format_and_instruction() {
        let payload = asr_payload_openai_audio("gpt-audio-1.5", "QUJD", "Transcribe this audio.");
        assert_eq!(
            payload
                .pointer("/messages/0/content/0/text")
                .and_then(|v| v.as_str()),
            Some("Transcribe this audio.")
        );
        let audio = payload
            .pointer("/messages/0/content/1/input_audio")
            .unwrap();
        assert_eq!(
            audio.pointer("/data").and_then(|v| v.as_str()),
            Some("QUJD")
        );
        assert_eq!(
            audio.pointer("/format").and_then(|v| v.as_str()),
            Some("wav")
        );
        // No top-level asr_options in the OpenAI-audio dialect.
        assert!(payload.pointer("/asr_options").is_none());
    }

    #[test]
    fn doubao_instruction_is_chinese_transcribe_only() {
        assert!(chat_audio_instruction("doubao_ark").contains("仅以文字形式返回识别结果"));
        assert_eq!(chat_audio_instruction("openai"), "Transcribe this audio.");
    }

    #[test]
    fn dialect_parsing_maps_known_values_and_defaults() {
        assert_eq!(
            CloudDialect::from_config("chat_asr_data_url"),
            CloudDialect::ChatAsrDataUrl
        );
        assert_eq!(
            CloudDialect::from_config("chat_openai_audio"),
            CloudDialect::ChatOpenaiAudio
        );
        assert_eq!(
            CloudDialect::from_config("openai_transcribe"),
            CloudDialect::OpenaiTranscribe
        );
        // Unknown / corrupt values fall back to the safe default.
        assert_eq!(
            CloudDialect::from_config("nonsense"),
            CloudDialect::ChatAsrDataUrl
        );
        assert_eq!(CloudDialect::from_config(""), CloudDialect::ChatAsrDataUrl);
    }

    #[test]
    fn dialect_serializes_to_the_config_strings() {
        assert_eq!(
            serde_json::to_string(&CloudDialect::ChatAsrDataUrl).unwrap(),
            "\"chat_asr_data_url\""
        );
        assert_eq!(
            serde_json::to_string(&CloudDialect::ChatOpenaiAudio).unwrap(),
            "\"chat_openai_audio\""
        );
        assert_eq!(
            serde_json::to_string(&CloudDialect::OpenaiTranscribe).unwrap(),
            "\"openai_transcribe\""
        );
    }

    #[test]
    fn urls_normalize_trailing_slash_and_known_suffix() {
        assert_eq!(
            chat_completions_url("https://api.xiaomimimo.com/v1"),
            "https://api.xiaomimimo.com/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("https://api.xiaomimimo.com/v1/"),
            "https://api.xiaomimimo.com/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("https://api.openai.com/v1/chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            transcriptions_url("https://api.openai.com/v1"),
            "https://api.openai.com/v1/audio/transcriptions"
        );
        assert_eq!(
            transcriptions_url("https://api.openai.com/v1/"),
            "https://api.openai.com/v1/audio/transcriptions"
        );
        assert_eq!(
            transcriptions_url("https://api.openai.com/v1/audio/transcriptions"),
            "https://api.openai.com/v1/audio/transcriptions"
        );
    }

    #[test]
    fn audio_size_guard_rejects_empty_and_oversized() {
        assert!(check_audio_len(&[0u8; 10]).is_err());
        assert!(check_audio_len(&[0u8; 44]).is_ok());
        assert!(check_audio_len(&vec![0u8; 7 * 1024 * 1024 + 1]).is_err());
    }

    #[test]
    fn call_struct_is_constructible_for_all_dialects() {
        for dialect in [
            CloudDialect::ChatAsrDataUrl,
            CloudDialect::ChatOpenaiAudio,
            CloudDialect::OpenaiTranscribe,
        ] {
            let c = call("m", dialect, "i");
            assert_eq!(c.dialect, dialect);
        }
    }
}
