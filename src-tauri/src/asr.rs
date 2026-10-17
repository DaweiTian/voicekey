//! MiMo cloud speech recognition (ASR) over the OpenAI-compatible
//! `chat/completions` endpoint (`mimo-v2.5-asr`), plus the shared WAV encoder.
//! Local ONNX inference (SenseVoice / X-ASR) and its model management were
//! removed in 0.5.11 — cloud is the only engine.
use base64::Engine;

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

/// Validate captured audio and encode it as an inline `data:` URL.
fn audio_data_url(audio_wav: &[u8]) -> Result<String, String> {
    if audio_wav.len() < 44 {
        return Err("录音为空，请重试。".into());
    }

    // MiMo ASR limit: base64 payload <= 10MB.
    if audio_wav.len() > 7 * 1024 * 1024 {
        return Err("录音过长，云端识别上限约 7MB，请缩短后重试。".into());
    }

    let b64 = base64::engine::general_purpose::STANDARD.encode(audio_wav);
    Ok(format!("data:audio/wav;base64,{b64}"))
}

/// Shared request body for the MiMo ASR chat-completions endpoint.
fn asr_payload(model: &str, data_url: &str, language: &str) -> serde_json::Value {
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

/// Cloud ASR via MiMo (OpenAI-compatible `mimo-v2.5-asr`).
/// Audio is a WAV/MP3 buffer; returns the recognized transcript.
/// Non-streaming; kept as the fallback path for `transcribe_cloud_stream`.
pub async fn transcribe_cloud(
    api_key: &str,
    base_url: &str,
    model: &str,
    audio_wav: &[u8],
    language: &str,
) -> Result<String, String> {
    if api_key.trim().is_empty() {
        return Err("MiMo API Key 为空，请到「设置 - 云端识别」填写。".into());
    }
    let data_url = audio_data_url(audio_wav)?;
    let url = chat_completions_url(base_url);
    let payload = asr_payload(model, &data_url, language);

    let response = crate::state::shared_http_client()
        .post(&url)
        .timeout(std::time::Duration::from_secs(120))
        .bearer_auth(api_key)
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

/// Cloud ASR via MiMo with SSE streaming (`"stream": true`).
///
/// `on_partial` receives the cumulative transcript so far every time the server
/// emits a `choices[].delta.content` increment, so the caller can surface live
/// progress (HUD) while the model is still generating. The non-streaming
/// `transcribe_cloud` remains the fallback: this function returns `Err` on
/// transport failures, HTTP errors, malformed SSE or empty output so callers
/// can retry once without streaming.
pub async fn transcribe_cloud_stream(
    api_key: &str,
    base_url: &str,
    model: &str,
    audio_wav: &[u8],
    language: &str,
    mut on_partial: impl FnMut(&str),
) -> Result<String, String> {
    if api_key.trim().is_empty() {
        return Err("MiMo API Key 为空，请到「设置 - 云端识别」填写。".into());
    }
    let data_url = audio_data_url(audio_wav)?;
    let url = chat_completions_url(base_url);
    let mut payload = asr_payload(model, &data_url, language);
    payload["stream"] = serde_json::Value::Bool(true);

    let response = crate::state::shared_http_client()
        .post(&url)
        .timeout(std::time::Duration::from_secs(120))
        .bearer_auth(api_key)
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
