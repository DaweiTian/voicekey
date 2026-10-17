use crate::config::model_storage_dir;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrFileMeta {
    pub name: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub urls: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AsrStatus {
    pub ready: bool,
    pub downloading: bool,
    pub mode: String,
    pub model_dir: String,
    pub message: String,
}

fn classic_files() -> Vec<AsrFileMeta> {
    vec![
        AsrFileMeta {
            name: "model.int8.onnx".into(),
            // MUST equal the real byte count. `status()` (readiness) and
            // `download_one()` (post-download validation) both compare with
            // **strict equality**, so an estimate here reports "not downloaded"
            // forever and makes every download attempt fail. Value verified
            // against the downloaded artifact on 2026-10-17 (SHA-256 c71f0ce0…,
            // 239_233_841 bytes) - update together with `sha256` when the model
            // revision changes.
            size_bytes: 239_233_841,
            sha256: "c71f0ce00bec95b07744e116345e33d8cbbe08cef896382cf907bf4b51a2cd51".into(),
            urls: vec![
                "https://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/main/model.int8.onnx".into(),
                "https://hf-mirror.com/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/main/model.int8.onnx".into(),
            ],
        },
        AsrFileMeta {
            name: "tokens.txt".into(),
            // Real byte count (same strict-equality rule as above); verified
            // 2026-10-17, SHA-256 f449eb28….
            size_bytes: 315_894,
            sha256: "f449eb28dc567533d7fa59be34e2abca8784f771850c78a47fb731a31429a1dc".into(),
            urls: vec![
                "https://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/main/tokens.txt".into(),
                "https://hf-mirror.com/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/main/tokens.txt".into(),
            ],
        },
    ]
}

fn streaming_files() -> Vec<AsrFileMeta> {
    vec![
        AsrFileMeta {
            name: "encoder-480ms.onnx".into(),
            // Real byte count of the revision this entry PINS via `sha256`
            // below (0c345403…abc74), verified on the machine's on-disk copy
            // (full 64-hex SHA-256 match, 592_968_361 bytes) and independently
            // by the primary source: a 2-byte range request to the modelscope
            // URL returns `Content-Range: bytes 0-1/592968361`, and its 302
            // redirects to the content-addressed LFS object
            // `0c/34/54033d249081df124ddcd7adaf3deca07d0b999b26f2ee5d2475d37abc74`
            // (i.e. exactly this `sha256`).
            //
            // Upstream drift (recorded, 2026-10-17): the author also published a
            // NEWER revision of this asset at the same URL - sampled at
            // 487_029_601 bytes / SHA-256 cb58274b…cf52f - and the two mirrors
            // were not always in step. A file from that newer revision is
            // REJECTED by `download_one`'s SHA-256 check, which is the intended
            // behaviour: the constant must always describe the pinned revision,
            // never "whatever upstream serves today". When the pin is upgraded,
            // change `sha256` and `size_bytes` together.
            size_bytes: 592_968_361,
            sha256: "0c3454033d249081df124ddcd7adaf3deca07d0b999b26f2ee5d2475d37abc74".into(),
            urls: vec![
                "https://www.modelscope.ai/models/Gilgamesh-J/X-ASR-zh-en/resolve/master/deployment/models/chunk-480ms-model/encoder-480ms.onnx".into(),
                "https://huggingface.co/GilgameshWind/X-ASR-zh-en/resolve/689ff18c584d29910da37b6fe904db0c1489c9d1/deployment/models/chunk-480ms-model/encoder-480ms.onnx".into(),
            ],
        },
        AsrFileMeta {
            name: "decoder-480ms.onnx".into(),
            // Real byte count, verified by download + SHA-256 match on
            // 2026-10-17 (was an estimate of 40_000_000).
            size_bytes: 11_309_084,
            sha256: "3658368d274a5d5fd39a7ac20c46bed0ad9cfea1f0feddef30d5d89797c1f499".into(),
            urls: vec![
                "https://www.modelscope.ai/models/Gilgamesh-J/X-ASR-zh-en/resolve/master/deployment/models/chunk-480ms-model/decoder-480ms.onnx".into(),
                "https://huggingface.co/GilgameshWind/X-ASR-zh-en/resolve/689ff18c584d29910da37b6fe904db0c1489c9d1/deployment/models/chunk-480ms-model/decoder-480ms.onnx".into(),
            ],
        },
        AsrFileMeta {
            name: "joiner-480ms.onnx".into(),
            // Real byte count, verified by download + SHA-256 match on
            // 2026-10-17 (was an estimate of 60_000_000).
            size_bytes: 10_260_467,
            sha256: "03781c98165a2385024c9cecdd2b6b13310d81db23a62c7da420782c2915cf81".into(),
            urls: vec![
                "https://www.modelscope.ai/models/Gilgamesh-J/X-ASR-zh-en/resolve/master/deployment/models/chunk-480ms-model/joiner-480ms.onnx".into(),
                "https://huggingface.co/GilgameshWind/X-ASR-zh-en/resolve/689ff18c584d29910da37b6fe904db0c1489c9d1/deployment/models/chunk-480ms-model/joiner-480ms.onnx".into(),
            ],
        },
        AsrFileMeta {
            name: "tokens.txt".into(),
            // Real byte count, verified by download + SHA-256 match on
            // 2026-10-17 (was an estimate of 1_000_000).
            size_bytes: 58_806,
            sha256: "b818a60878b9aae978cbb8ad594acbd403d76d1af2e31ef4197c84e2dbdba27c".into(),
            urls: vec![
                "https://www.modelscope.ai/models/Gilgamesh-J/X-ASR-zh-en/resolve/master/deployment/models/chunk-480ms-model/tokens.txt".into(),
                "https://huggingface.co/GilgameshWind/X-ASR-zh-en/resolve/689ff18c584d29910da37b6fe904db0c1489c9d1/deployment/models/chunk-480ms-model/tokens.txt".into(),
            ],
        },
    ]
}

pub fn classic_dir() -> PathBuf {
    model_storage_dir()
        .join("sensevoice")
        .join("models")
        .join("v1")
}

pub fn streaming_dir() -> PathBuf {
    model_storage_dir()
        .join("x-asr-480ms")
        .join("models")
        .join("v1")
}

fn files_for(mode: &str) -> Vec<AsrFileMeta> {
    if mode == "streaming" {
        streaming_files()
    } else {
        classic_files()
    }
}

fn dir_for(mode: &str) -> PathBuf {
    if mode == "streaming" {
        streaming_dir()
    } else {
        classic_dir()
    }
}

fn verify_sha256(path: &Path, expected: &str) -> Result<(), String> {
    let data = fs::read(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let actual = hex::encode(hasher.finalize());
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(format!(
            "checksum mismatch for {}: expected {expected}, got {actual}",
            path.display()
        ))
    }
}

fn status(mode: &str) -> AsrStatus {
    let dir = dir_for(mode);
    let files = files_for(mode);
    let ready = !files.is_empty()
        && files.iter().all(|f| {
            let p = dir.join(&f.name);
            p.exists()
                && fs::metadata(&p)
                    .map(|m| m.len() == f.size_bytes)
                    .unwrap_or(false)
        });
    AsrStatus {
        ready,
        downloading: false,
        mode: mode.to_string(),
        model_dir: dir.display().to_string(),
        message: if ready {
            "Model ready".into()
        } else {
            "Local model not downloaded. You can use cloud ASR (MiMo) instead, or download the model from the Settings page.".into()
        },
    }
}

pub fn get_status(mode: &str) -> AsrStatus {
    status(mode)
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

/// Download missing local model assets with HTTPS + SHA-256 + size verification.
pub async fn download(mode: &str) -> Result<AsrStatus, String> {
    let dir = dir_for(mode);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let files = files_for(mode);
    if files.is_empty() {
        return Err("No model assets configured".into());
    }

    // Shared pool; the 300s download budget moves to the per-request timeout.
    let client = crate::state::shared_http_client();

    for file in files {
        let target = dir.join(&file.name);
        if target.exists()
            && fs::metadata(&target)
                .map(|m| m.len() == file.size_bytes)
                .unwrap_or(false)
        {
            continue;
        }
        let _ = fs::remove_file(&target);

        let mut downloaded = false;
        let mut last_error = String::from("no sources");
        for url in &file.urls {
            match download_one(client, url, &target, &file).await {
                Ok(()) => {
                    downloaded = true;
                    break;
                }
                Err(e) => {
                    last_error = e;
                    let _ = fs::remove_file(&target);
                }
            }
        }
        if !downloaded {
            return Err(format!(
                "Failed to download {} from Hugging Face / ModelScope mirrors: {last_error}",
                file.name
            ));
        }
    }

    Ok(status(mode))
}

async fn download_one(
    client: &reqwest::Client,
    url: &str,
    target: &Path,
    file: &AsrFileMeta,
) -> Result<(), String> {
    let parsed = url::Url::parse(url).map_err(|e| e.to_string())?;
    if parsed.scheme() != "https" {
        return Err("Model downloads must use HTTPS".into());
    }

    let response = client
        .get(url)
        .timeout(std::time::Duration::from_secs(300))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;

    let bytes = response.bytes().await.map_err(|e| e.to_string())?;
    if bytes.len() as u64 != file.size_bytes {
        log::warn!(
            "size mismatch for {}: expected {}, got {}",
            file.name,
            file.size_bytes,
            bytes.len()
        );
    }

    let tmp = target.with_extension("download");
    fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    verify_sha256(&tmp, &file.sha256)?;
    if bytes.len() as u64 != file.size_bytes {
        let _ = fs::remove_file(&tmp);
        return Err(format!(
            "unexpected size for {}: {} != {}",
            file.name,
            bytes.len(),
            file.size_bytes
        ));
    }
    fs::rename(&tmp, target).map_err(|e| e.to_string())
}

pub fn delete(mode: &str) -> Result<AsrStatus, String> {
    let dir = dir_for(mode);
    let root = model_storage_dir();
    let dir_canon = fs::canonicalize(&dir).unwrap_or_else(|_| dir.clone());
    let root_canon = fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
    if !dir_canon.starts_with(&root_canon) {
        return Err("Refusing to delete a directory outside managed ASR storage".into());
    }
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    Ok(status(mode))
}

use base64::Engine;
