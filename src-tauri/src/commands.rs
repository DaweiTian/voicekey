use crate::asr;
use crate::audio::{
    AudioChunkPayload, SessionStatus, StreamingAudioEndPayload, StreamingAudioFramePayload,
};
use crate::config::{AppConfig, ConfigSecretRequest};
use crate::history::HistoryManager;
use crate::hotkey;
use crate::inject;
use crate::llm;
use crate::platform;
use crate::state::AppState;
use crate::updater;
use crate::updater::UpdateInfo;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

fn only_main_window(window: &WebviewWindow, action: &str) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err(format!(
            "{action} is only available from the settings window"
        ))
    }
}

fn mimo_asr_credentials(config: &AppConfig) -> (String, String, String) {
    // Dedicated ASR credentials win; fall back to the shared MiMo LLM connection.
    let key = if !config.asr.mimo_api_key.trim().is_empty() {
        config.asr.mimo_api_key.clone()
    } else if !config.llm_refine.mimo.api_key.trim().is_empty() {
        config.llm_refine.mimo.api_key.clone()
    } else {
        config.llm_refine.api_key.clone()
    };
    let base = if !config.asr.mimo_endpoint.trim().is_empty() {
        config.asr.mimo_endpoint.clone()
    } else if !config.llm_refine.mimo.endpoint.trim().is_empty() {
        config.llm_refine.mimo.endpoint.clone()
    } else {
        "https://api.xiaomimimo.com/v1".into()
    };
    let model = if config.asr.mimo_model.trim().is_empty() {
        "mimo-v2.5-asr".into()
    } else {
        config.asr.mimo_model.clone()
    };
    (key, base, model)
}

/// Show the HUD overlay window (recording / processing feedback).
fn overlay_show<R: tauri::Runtime>(app: &AppHandle<R>) {
    if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.show();
    }
}

/// Hide the HUD overlay window immediately.
fn overlay_hide<R: tauri::Runtime>(app: &AppHandle<R>) {
    if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.hide();
    }
}

/// Hide the HUD overlay window after a short delay.
///
/// The delay lets the user read the final success / error state. We re-check
/// the session status before hiding so a newly started session is never hidden.
fn overlay_hide_later<R: tauri::Runtime>(app: &AppHandle<R>) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
        let active = handle
            .state::<AppState>()
            .session
            .lock()
            .map(|s| {
                matches!(
                    s.status,
                    SessionStatus::Recording | SessionStatus::Processing
                )
            })
            .unwrap_or(false);
        if !active {
            if let Some(win) = handle.get_webview_window("overlay") {
                let _ = win.hide();
            }
        }
    });
}

/// Record and broadcast a session error, reset the state machine to `Idle`
/// (so it can never get stuck in `Processing`), surface it on the HUD, and
/// schedule the overlay to hide. Every stop failure path funnels through here.
fn emit_session_error<R: tauri::Runtime>(app: &AppHandle<R>, state: &AppState, message: String) {
    if let Ok(mut session) = state.session.lock() {
        session.fail(message.clone());
        // `fail` marks Error; reset to Idle so the next PTT press is accepted
        // and the session is never left in Processing.
        session.status = SessionStatus::Idle;
    }
    let _ = app.emit("error", serde_json::json!({ "message": message.clone() }));
    let _ = app.emit(
        "overlay-update",
        serde_json::json!({ "status": "error", "message": message }),
    );
    overlay_hide_later(app);
}

/// Wait until the recorder confirms its final flush (end marker) or the
/// timeout elapses. Avoids both truncating the tail (too short a fixed sleep)
/// and always paying the worst case when the marker arrives early.
async fn wait_for_flush(state: &AppState, timeout_ms: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    loop {
        let done = state
            .session
            .lock()
            .map(|s| s.streaming_final || s.final_chunk_seen)
            .unwrap_or(true);
        if done || std::time::Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Shared finalize path used by the stop command and PTT release.
///
/// Wraps the fallible work so that *every* failure path (empty key, non-2xx
/// HTTP, empty transcript, no audio, local engine not wired up) emits an
/// `error` event, updates the HUD and resets the session state.
pub async fn stop_session_internal<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Result<String, String> {
    match stop_session_inner(app).await {
        Ok(text) => Ok(text),
        Err(err) => {
            let state = app.state::<AppState>();
            emit_session_error(app, &state, err.clone());
            Err(err)
        }
    }
}

async fn stop_session_inner<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<String, String> {
    let state = app.state::<AppState>();

    {
        let mut session = state.session.lock().map_err(|e| e.to_string())?;
        if session.status != SessionStatus::Recording {
            return Err("当前没有正在进行的录音。".into());
        }
        session.status = SessionStatus::Processing;
    }

    // Ask the recorder window to stop and flush a final PCM/chunk marker.
    let session_id = {
        let session = state.session.lock().map_err(|e| e.to_string())?;
        session.id.clone()
    };
    let _ = app.emit("stop-recording", session_id.clone());
    let _ = app.emit(
        "overlay-update",
        serde_json::json!({ "status": "processing" }),
    );

    // Wait for the recorder to flush its tail. The worklet flush waits up to
    // 1000ms (AudioRecorder.tsx), so allow a little more but return as soon as
    // the final marker arrives instead of always sleeping the full duration.
    wait_for_flush(&state, 1200).await;

    let config = state.config.load();
    let wav = {
        let mut session = state.session.lock().map_err(|e| e.to_string())?;
        session.take_pcm_wav()
    };

    // Transcribe
    let raw_text = if wav.len() > 44 {
        match config.asr.engine.as_str() {
            "local" => {
                let st = asr::get_status(if config.asr.streaming_enabled {
                    "streaming"
                } else {
                    "classic"
                });
                if !st.ready {
                    return Err(
                        "请先下载识别模型，或在设置中把识别引擎切换为「MiMo 云端识别」。".into(),
                    );
                }
                // Local ONNX runtime is not linked in this Tauri build yet.
                return Err("本地识别运行时尚未接入，请使用 MiMo 云端识别（设置 → 识别引擎）。".into());
            }
            _ => {
                // Default: MiMo cloud ASR
                let (api_key, base, model) = mimo_asr_credentials(&config);
                if api_key.trim().is_empty() {
                    return Err("MiMo API Key 为空，请到「设置 - 云端识别」填写。".into());
                }
                asr::transcribe_cloud(&api_key, &base, &model, &wav, "auto").await?
            }
        }
    } else {
        // Fall back to any classic chunk blobs if PCM was empty.
        let chunks = {
            let session = state.session.lock().map_err(|e| e.to_string())?;
            let mut all = Vec::new();
            for (_idx, buf) in session.chunks.iter() {
                all.extend_from_slice(buf);
            }
            all
        };
        if chunks.is_empty() {
            return Err("没有录到声音，请检查麦克风后重试。".into());
        }
        // If the recorder sent wav/webm chunks, try cloud ASR on the first non-empty one
        // after wrapping wav or using raw if already wav.
        let audio = chunks;
        let (api_key, base, model) = mimo_asr_credentials(&config);
        if api_key.trim().is_empty() {
            return Err("MiMo API Key 为空，请到「设置 - 云端识别」填写。".into());
        }
        asr::transcribe_cloud(&api_key, &base, &model, &audio, "auto").await?
    };

    let mut final_text = raw_text;

    // Optional refine / translate via LLM
    if !final_text.trim().is_empty()
        && (config.llm_refine.enabled || config.llm_refine.translate_output)
    {
        match llm::refine_text(&final_text, &config.llm_refine).await {
            Ok(processed) if !processed.trim().is_empty() => final_text = processed,
            Ok(_) => {}
            Err(e) => {
                log::warn!("refine failed, using raw transcript: {e}");
            }
        }
    }

    if !final_text.trim().is_empty() {
        let _ = HistoryManager::add(final_text.clone());
        if let Err(e) = inject::inject_text(&final_text) {
            log::warn!("text injection failed: {e}");
        }
    }

    {
        let mut session = state.session.lock().map_err(|e| e.to_string())?;
        session.complete(final_text.clone());
    }

    let _ = app.emit(
        "overlay-update",
        serde_json::json!({
            "status": "success",
            "transcript": final_text,
        }),
    );
    let _ = app.emit("transcription", final_text.clone());
    overlay_hide_later(app);
    Ok(final_text)
}

pub fn start_session_internal<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<Value, String> {
    let state = app.state::<AppState>();
    let config = state.config.load();
    // Prefer streaming capture so we always get PCM for cloud ASR.
    let payload = {
        let mut session = state.session.lock().map_err(|e| e.to_string())?;
        if session.status == SessionStatus::Recording {
            return Err("Already recording".into());
        }
        session.start(true)
    };
    let mut payload = payload;
    payload.microphone_device_id = if config.asr.microphone_device_id.is_empty() {
        None
    } else {
        Some(config.asr.microphone_device_id.clone())
    };
    payload.low_volume_mode = config.asr.low_volume_mode;
    // asrMode: 'streaming' routes AudioRecorder to PCM worklet (best for cloud ASR).
    payload.asr_mode = "streaming".into();
    let _ = app.emit("start-recording", &payload);
    // Show the HUD and switch it to the recording state.
    overlay_show(app);
    let _ = app.emit(
        "overlay-update",
        serde_json::json!({ "status": "recording" }),
    );
    serde_json::to_value(&payload).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_config(state: State<AppState>) -> AppConfig {
    state.config.load_masked()
}

#[tauri::command]
pub fn set_config(
    app: AppHandle,
    state: State<AppState>,
    config: Value,
) -> Result<AppConfig, String> {
    // Keep the previously working accelerators so we can roll back if the new
    // ones cannot be registered (e.g. the OS rejects a key).
    let previous_hotkeys = state.config.load().hotkey;
    let merged = state.config.merge(&config)?;
    if let Err(err) = hotkey::rebind(&app, &merged.hotkey) {
        // Re-bind the previous accelerators so PTT keeps working, and restore
        // the stored hotkey config to match the active bindings.
        let _ = hotkey::rebind(&app, &previous_hotkeys);
        let _ = state.config.merge(&serde_json::json!({
            "hotkey": {
                "pttKey": previous_hotkeys.ptt_key,
                "toggleSettings": previous_hotkeys.toggle_settings,
                "translateKey": previous_hotkeys.translate_key,
            }
        }));
        let _ = app.emit(
            "error",
            serde_json::json!({ "message": format!("快捷键不受支持，已恢复为原设置：{err}") }),
        );
    }
    Ok(state.config.load_masked())
}

#[tauri::command]
pub fn get_config_secret(
    window: WebviewWindow,
    state: State<AppState>,
    request: ConfigSecretRequest,
) -> Result<String, String> {
    only_main_window(&window, "Saved API key access")?;
    state.config.get_secret(&request)
}

#[tauri::command]
pub async fn test_refine_connection(
    state: State<'_, AppState>,
    config: Value,
) -> Result<llm::RefineConnectionResult, String> {
    if config.is_object() {
        let payload = if config.get("llmRefine").is_some() {
            config.clone()
        } else {
            serde_json::json!({ "llmRefine": config })
        };
        state.config.merge(&payload)?;
    }
    let merged = state.config.load();
    Ok(llm::test_connection(&merged.llm_refine).await)
}

#[tauri::command]
pub async fn connect_token_dance() -> Result<Value, String> {
    Err(
        "TokenDance browser authorization is not available in the Tauri build yet. Paste an API key manually."
            .into(),
    )
}

#[tauri::command]
pub fn get_local_asr_status() -> asr::AsrStatus {
    asr::get_status("classic")
}

#[tauri::command]
pub async fn download_local_asr() -> Result<asr::AsrStatus, String> {
    asr::download("classic").await
}

#[tauri::command]
pub fn delete_local_asr(window: WebviewWindow) -> Result<asr::AsrStatus, String> {
    only_main_window(&window, "Local ASR model deletion")?;
    asr::delete("classic")
}

#[tauri::command]
pub fn get_streaming_asr_status() -> asr::AsrStatus {
    asr::get_status("streaming")
}

#[tauri::command]
pub async fn download_streaming_asr() -> Result<asr::AsrStatus, String> {
    asr::download("streaming").await
}

#[tauri::command]
pub fn delete_streaming_asr(window: WebviewWindow) -> Result<asr::AsrStatus, String> {
    only_main_window(&window, "Streaming ASR model deletion")?;
    asr::delete("streaming")
}

#[tauri::command]
pub fn open_asr_model_directory(window: WebviewWindow, mode: String) -> Result<(), String> {
    only_main_window(&window, "ASR model directory access")?;
    let dir = if mode == "streaming" {
        asr::streaming_dir()
    } else if mode == "classic" {
        asr::classic_dir()
    } else {
        return Err("Invalid ASR model directory type".into());
    };
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    open_path(&dir)
}

fn open_path(path: &PathBuf) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(path)
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[tauri::command]
pub fn get_host_capabilities() -> Value {
    serde_json::json!({
        "available": true,
        "logicalCpuCount": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4),
        "memoryBytes": 16u64 * 1024 * 1024 * 1024,
        "meetsStreamingAsrRecommendation": true,
    })
}

#[tauri::command]
pub fn get_app_language(state: State<AppState>) -> Value {
    let language = state.language.lock().unwrap().clone();
    serde_json::json!({ "language": language, "source": "config" })
}

#[tauri::command]
pub fn set_app_language(
    app: AppHandle,
    state: State<AppState>,
    language: String,
) -> Result<(), String> {
    {
        let mut lang = state.language.lock().unwrap();
        *lang = language.clone();
    }
    let _ = state.config.merge(&serde_json::json!({
        "app": { "language": language }
    }));
    let _ = app.emit(
        "app-language-changed",
        serde_json::json!({
            "language": state.language.lock().unwrap().clone(),
            "source": "config"
        }),
    );
    Ok(())
}

#[tauri::command]
pub fn get_history() -> Vec<crate::history::HistoryItem> {
    HistoryManager::get_all()
}

#[tauri::command]
pub fn clear_history() -> Result<(), String> {
    HistoryManager::clear()
}

#[tauri::command]
pub fn delete_history_item(id: String) -> Result<bool, String> {
    HistoryManager::delete(&id)
}

#[tauri::command]
pub fn start_session(app: AppHandle) -> Result<Value, String> {
    start_session_internal(&app)
}

#[tauri::command]
pub async fn stop_session(app: AppHandle) -> Result<String, String> {
    stop_session_internal(&app).await
}

#[tauri::command]
pub fn cancel_session(app: AppHandle, state: State<AppState>) -> Result<(), String> {
    let mut session = state.session.lock().map_err(|e| e.to_string())?;
    session.cancel();
    let _ = app.emit("stop-recording", session.id.clone());
    drop(session);
    overlay_hide(&app);
    Ok(())
}

#[tauri::command]
pub fn get_session_status(state: State<AppState>) -> String {
    state
        .session
        .lock()
        .map(|s| s.status_name())
        .unwrap_or_else(|_| "error".into())
}

#[tauri::command]
pub fn send_audio_chunk(state: State<AppState>, payload: AudioChunkPayload) -> Result<(), String> {
    if payload.chunk_index > 10_000 || payload.mime_type.len() > 128 {
        return Err("Invalid audio chunk".into());
    }
    let mut session = state.session.lock().map_err(|e| e.to_string())?;
    let _ = session.push_chunk(payload);
    Ok(())
}

#[tauri::command]
pub fn send_streaming_audio_frame(
    state: State<AppState>,
    payload: StreamingAudioFramePayload,
) -> Result<(), String> {
    if payload.buffer.is_empty() || payload.buffer.len() > 1024 * 1024 {
        return Err("Invalid streaming frame".into());
    }
    let mut session = state.session.lock().map_err(|e| e.to_string())?;
    if !session.push_streaming_frame(payload) {
        return Err("Rejected streaming frame".into());
    }
    Ok(())
}

#[tauri::command]
pub fn send_streaming_audio_end(
    app: AppHandle,
    state: State<AppState>,
    payload: StreamingAudioEndPayload,
) -> Result<(), String> {
    let mut session = state.session.lock().map_err(|e| e.to_string())?;
    if !session.finish_streaming(payload) {
        return Err("Rejected streaming end".into());
    }
    let _ = app.emit("streaming-final", session.id.clone());
    Ok(())
}

#[tauri::command]
pub fn send_error(app: AppHandle, state: State<AppState>, error: String) -> Result<(), String> {
    // Same channel shape and reset semantics as other failure paths.
    emit_session_error(&app, &state, error);
    Ok(())
}

#[tauri::command]
pub fn send_audio_level(app: AppHandle, level: f32) {
    let _ = app.emit("audio-level", level);
}

#[tauri::command]
pub async fn check_for_updates() -> UpdateInfo {
    updater::check_for_updates().await
}

#[tauri::command]
pub fn get_update_status() -> Option<UpdateInfo> {
    None
}

#[tauri::command]
pub fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
pub fn open_external(url: String) -> Result<(), String> {
    let safe = updater::sanitize_external_url(&url)?;
    open_path(&PathBuf::from(safe))
}

#[tauri::command]
pub fn get_log_tail() -> Result<String, String> {
    let dir = crate::config::log_dir();
    let path = dir.join("voice-key.log");
    match fs::read(&path) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes).to_string();
            let tail: String = text
                .chars()
                .rev()
                .take(200_000)
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            Ok(tail)
        }
        Err(_) => Ok(String::new()),
    }
}

#[tauri::command]
pub fn open_log_folder() -> Result<(), String> {
    let dir = crate::config::log_dir();
    open_path(&dir)
}

#[tauri::command]
pub fn log(entry: Value) -> Result<(), String> {
    let dir = crate::config::log_dir();
    let path = dir.join("voice-key.log");
    let level = entry.get("level").and_then(|v| v.as_str()).unwrap_or("info");
    let message = entry
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .replace('\n', "\\n")
        .replace('\r', "\\r");
    let line = format!(
        "{} [{level}] {message}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f")
    );
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(line.as_bytes()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_linux_integration_status() -> platform::LinuxIntegrationStatus {
    platform::linux_integration_status()
}

#[tauri::command]
pub fn install_linux_integration() -> platform::LinuxIntegrationStatus {
    platform::install_linux_integration()
}

#[tauri::command]
pub fn remove_linux_integration() -> platform::LinuxIntegrationStatus {
    platform::remove_linux_integration()
}

#[tauri::command]
pub fn set_ignore_mouse_events(window: WebviewWindow, ignore: bool) -> Result<(), String> {
    window
        .set_ignore_cursor_events(ignore)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn show_settings_window(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.show();
        let _ = win.set_focus();
    }
    Ok(())
}

#[tauri::command]
pub async fn refine_text(state: State<'_, AppState>, text: String) -> Result<String, String> {
    let config = state.config.load();
    llm::refine_text(&text, &config.llm_refine).await
}

#[tauri::command]
pub async fn translate_text(
    state: State<'_, AppState>,
    text: String,
    target_language: String,
) -> Result<String, String> {
    let config = state.config.load();
    llm::translate_text(&text, &target_language, &config.llm_refine).await
}
