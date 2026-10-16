use crate::asr;
use crate::audio::{
    now_ms, session_time_limit_reached, AudioChunkPayload, SessionStatus, StreamingAudioEndPayload,
    StreamingAudioFramePayload, SESSION_MAX_DURATION_SECONDS,
};
use crate::config::{AppConfig, ConfigSecretRequest};
use crate::history::HistoryManager;
use crate::hotkey;
use crate::inject;
use crate::llm;
use crate::platform;
use crate::state::AppState;
use crate::stats::{StatsManager, StatsStore};
use crate::updater;
use crate::updater::UpdateInfo;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Monitor, State, WebviewWindow,
};

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

/// Gap between the HUD's bottom edge and the bottom of the work area
/// (i.e. above the taskbar), in logical pixels.
const OVERLAY_BOTTOM_MARGIN: f64 = 48.0;

/// Gap between the HUD capsule and the work-area edge it snaps to when the
/// user drops a drag (`set_overlay_position`), in physical pixels (the
/// contract unit for drag positions).
const OVERLAY_EDGE_GAP: f64 = 10.0;

/// Fallback HUD size (logical px) when the window reports no size.
/// Keep in sync with the `overlay` entry in `tauri.conf.json`.
const OVERLAY_FALLBACK_SIZE: (f64, f64) = (280.0, 64.0);

/// Bounds for `set_overlay_size` (logical px). Clamped so a buggy or hostile
/// renderer value can never produce a tiny unusable or huge invisible capsule.
const OVERLAY_MIN_SIZE: (u32, u32) = (40, 32); // (width, height)
const OVERLAY_MAX_SIZE: (u32, u32) = (400, 120);

/// Work area (physical px) of `monitor`: origin + size, excluding the
/// taskbar / dock whenever the platform reports one.
fn monitor_work_area(monitor: &Monitor) -> (f64, f64, f64, f64) {
    let work = monitor.work_area();
    if work.size.width > 0 && work.size.height > 0 {
        (
            work.position.x as f64,
            work.position.y as f64,
            work.size.width as f64,
            work.size.height as f64,
        )
    } else {
        let position = monitor.position();
        let size = monitor.size();
        (
            position.x as f64,
            position.y as f64,
            size.width as f64,
            size.height as f64,
        )
    }
}

/// Geometry snapshot for placing the HUD on the monitor that owns a physical
/// desktop point. All values are physical (device) pixels taken from the *same*
/// monitor, so the math is multi-monitor and DPI safe.
struct OverlayFrame {
    area_x: f64,
    area_y: f64,
    area_w: f64,
    area_h: f64,
    window_w: f64,
    window_h: f64,
    monitor_scale: f64,
    window_scale: f64,
}

impl OverlayFrame {
    /// Resolve the work area and window size on the monitor containing
    /// `(x, y)` (falling back to the window's current monitor, then the
    /// primary monitor). `None` when the platform reports no monitor at all.
    fn resolve<R: tauri::Runtime>(
        app: &AppHandle<R>,
        win: &WebviewWindow<R>,
        x: f64,
        y: f64,
    ) -> Option<Self> {
        // `monitor_from_point` / monitor rects are both physical (device)
        // pixels on the desktop virtual screen, so they compare directly.
        let monitor = app
            .monitor_from_point(x, y)
            .ok()
            .flatten()
            .or_else(|| win.current_monitor().ok().flatten())
            .or_else(|| app.primary_monitor().ok().flatten())?;
        let monitor_scale = monitor.scale_factor();
        // The window is configured in logical pixels, so its logical size stays
        // constant; convert it to the physical size it has on `monitor`.
        let window_scale = win.scale_factor().unwrap_or(monitor_scale);
        let (logical_w, logical_h) = win
            .inner_size()
            .map(|size| {
                (
                    size.width as f64 / window_scale,
                    size.height as f64 / window_scale,
                )
            })
            .unwrap_or(OVERLAY_FALLBACK_SIZE);
        let (area_x, area_y, area_w, area_h) = monitor_work_area(&monitor);
        Some(Self {
            area_x,
            area_y,
            area_w,
            area_h,
            window_w: logical_w * monitor_scale,
            window_h: logical_h * monitor_scale,
            monitor_scale,
            window_scale,
        })
    }

    /// Clamp a physical position so the whole capsule stays inside the work
    /// area (an oversized capsule degrades to anchored at the area origin).
    fn clamp(&self, x: f64, y: f64) -> (f64, f64) {
        (
            x.clamp(
                self.area_x,
                (self.area_x + self.area_w - self.window_w).max(self.area_x),
            ),
            y.clamp(
                self.area_y,
                (self.area_y + self.area_h - self.window_h).max(self.area_y),
            ),
        )
    }

    /// Move the HUD to a physical desktop position.
    ///
    /// `set_position` converts logical to physical with the window's *current*
    /// scale factor (tao `set_outer_position`), so hand it logical coordinates
    /// derived from that same scale and the window lands exactly on (x, y).
    fn apply<R: tauri::Runtime>(&self, win: &WebviewWindow<R>, x: f64, y: f64) {
        let _ = win.set_position(LogicalPosition::new(
            x / self.window_scale,
            y / self.window_scale,
        ));
    }
}

/// Place the HUD horizontally centered on the monitor that contains the
/// cursor, near the bottom of that monitor's work area, above the taskbar.
/// All values are physical desktop pixels, taken from the same monitor, so
/// the math is multi-monitor and DPI safe.
fn position_overlay_bottom_center<R: tauri::Runtime>(app: &AppHandle<R>, win: &WebviewWindow<R>) {
    // The cursor only selects the target monitor; the HUD itself no longer
    // follows it.
    let Ok(cursor) = app.cursor_position() else {
        return;
    };
    let Some(frame) = OverlayFrame::resolve(app, win, cursor.x, cursor.y) else {
        return;
    };
    let margin = OVERLAY_BOTTOM_MARGIN * frame.monitor_scale;

    // Horizontal: work-area center, clamped so an oversized window still fits.
    let x = frame.area_x + (frame.area_w - frame.window_w) / 2.0;
    // Vertical: `OVERLAY_BOTTOM_MARGIN` above the work-area bottom (taskbar).
    let y = frame.area_y + frame.area_h - frame.window_h - margin;

    let (x, y) = frame.clamp(x, y);
    frame.apply(win, x, y);
}

/// Place the HUD at its persisted drag position. The position is re-clamped
/// into the work area of the monitor that owns it (monitor layouts change; the
/// capsule must never end up off-screen) but never re-snapped or re-centered:
/// this is what keeps "drag away, then resize / show" from springing back to
/// the bottom-center default.
fn position_overlay_persisted<R: tauri::Runtime>(
    app: &AppHandle<R>,
    win: &WebviewWindow<R>,
    x: f64,
    y: f64,
) {
    let Some(frame) = OverlayFrame::resolve(app, win, x, y) else {
        return;
    };
    let (x, y) = frame.clamp(x, y);
    frame.apply(win, x, y);
}

/// Place the HUD overlay: the persisted drag position when one is stored,
/// otherwise the bottom-center default. Used by every show / resize path.
fn position_overlay<R: tauri::Runtime>(app: &AppHandle<R>, win: &WebviewWindow<R>) {
    let persisted = app
        .try_state::<AppState>()
        .map(|s| s.config.load().app.overlay_position)
        .flatten();
    match persisted {
        Some(pos) => position_overlay_persisted(app, win, pos.x, pos.y),
        None => position_overlay_bottom_center(app, win),
    }
}

/// Snap a dropped physical position to the nearest edge of the owning
/// monitor's work area (the `set_overlay_position` contract): the edge nearest
/// to the dropped point wins and the capsule ends up [`OVERLAY_EDGE_GAP`]
/// physical pixels from it; the other axis is clamped into the work area and
/// the result is clamped on both axes so the capsule always fits on screen.
/// Moves the window to the snapped position and returns it (for persistence).
fn place_overlay_snapped<R: tauri::Runtime>(
    app: &AppHandle<R>,
    win: &WebviewWindow<R>,
    x: f64,
    y: f64,
) -> Option<(f64, f64)> {
    let frame = OverlayFrame::resolve(app, win, x, y)?;

    // Distance from the dropped point to each work-area edge (physical px).
    let d_left = (x - frame.area_x).abs();
    let d_right = (frame.area_x + frame.area_w - x).abs();
    let d_top = (y - frame.area_y).abs();
    let d_bottom = (frame.area_y + frame.area_h - y).abs();
    let nearest = d_left.min(d_right).min(d_top).min(d_bottom);

    let (mut nx, mut ny) = (x, y);
    if nearest == d_left {
        nx = frame.area_x + OVERLAY_EDGE_GAP;
    } else if nearest == d_right {
        nx = frame.area_x + frame.area_w - frame.window_w - OVERLAY_EDGE_GAP;
    } else if nearest == d_top {
        ny = frame.area_y + OVERLAY_EDGE_GAP;
    } else {
        ny = frame.area_y + frame.area_h - frame.window_h - OVERLAY_EDGE_GAP;
    }
    let (sx, sy) = frame.clamp(nx, ny);
    frame.apply(win, sx, sy);
    Some((sx, sy))
}

/// Whether the user *wants* the always-on HUD capsule visible (the tray
/// toggle's persisted intent, `app.overlayVisible`). Auto-show paths (startup,
/// session start) honor it: a capsule hidden from the tray stays hidden even
/// when a session starts (recording and the middle-click / keyboard triggers
/// keep working without it). The tray "Show floating capsule" entry passes
/// `force` to [`overlay_show`] and overrides this intent, so it can always
/// bring the capsule back even if the flag ever drifts from reality.
fn overlay_visible<R: tauri::Runtime>(app: &AppHandle<R>) -> bool {
    app.try_state::<AppState>()
        .map(|s| s.config.load().app.overlay_visible)
        .unwrap_or(true)
}

/// Persist the capsule visibility flag and keep the tray "Show floating
/// capsule" checkmark in sync with it. Called after every show / hide so
/// `app.overlayVisible` always matches the state actually reached.
///
/// Thread-safe to call from anywhere: the config write runs under
/// `config.rs::CONFIG_LOCK` (worker-thread callers serialize with every other
/// writer), and the muda menu mutation is dispatched to the main thread
/// (same pattern as `windowing::schedule_main_window_destroy`) because this
/// also runs on the overlay-rebuild worker thread (`overlay_show`).
fn sync_overlay_visibility_flag<R: tauri::Runtime>(app: &AppHandle<R>, visible: bool) {
    if let Some(state) = app.try_state::<AppState>() {
        let stored = state.config.load().app.overlay_visible;
        if stored != visible {
            if let Err(e) = state.config.merge(&serde_json::json!({
                "app": { "overlayVisible": visible }
            })) {
                log::warn!("overlay visibility persist failed: {e}");
            }
        }
    }
    if let Err(e) = app.run_on_main_thread(move || {
        crate::tray::set_overlay_menu_checked(visible);
    }) {
        log::warn!("overlay menu checkmark sync failed: {e}");
    }
}

/// Show the HUD overlay window (standby capsule / recording feedback).
///
/// The window is repositioned (persisted drag position, else bottom-center of
/// the cursor's monitor) *before* it becomes visible so it never flashes at
/// its previous position: both calls travel through the same event-loop queue,
/// in order.
///
/// The Windows hardening (no activation + no DWM focus frame) is re-applied
/// around the show, idempotently: `show()` is the only call that can activate
/// the HUD, so `WS_EX_NOACTIVATE` must already be in place *before* it runs,
/// and the border attribute is set again afterwards in case anything reset it.
/// This is also the fallback for a setup-time pass that found no overlay
/// window yet - the HUD can never be shown un-hardened. The startup standby
/// show goes through this same path, so the always-on capsule is hardened too.
///
/// `force` bypasses the user-intent guard (`app.overlayVisible`): only the
/// tray "Show floating capsule" entry uses it, so that entry is always able to
/// revive the capsule. A missing (destroyed) overlay window is rebuilt on a
/// worker thread - creating a webview window while blocking the main thread
/// deadlocks on Windows (wry#583) and this runs from tray menu events too.
fn overlay_show<R: tauri::Runtime>(app: &AppHandle<R>, force: bool) {
    if !force && !overlay_visible(app) {
        return;
    }
    if let Some(win) = app.get_webview_window(crate::windowing::OVERLAY_WINDOW_LABEL) {
        show_overlay_window(app, &win);
    } else {
        let handle = app.clone();
        std::thread::spawn(
            move || match crate::windowing::ensure_overlay_window(&handle) {
                Some(win) => show_overlay_window(&handle, &win),
                None => log::error!("overlay_show: overlay window is unavailable"),
            },
        );
    }
}

/// Harden, position and show one existing overlay window, then sync the
/// persisted visibility flag to what actually happened.
fn show_overlay_window<R: tauri::Runtime>(app: &AppHandle<R>, win: &WebviewWindow<R>) {
    crate::windowing::harden_overlay_window(win);
    position_overlay(app, win);
    if win.show().is_ok() {
        crate::windowing::harden_overlay_window(win);
        sync_overlay_visibility_flag(app, true);
    }
}

/// Hide the HUD overlay window immediately (tray "hide floating capsule" and
/// closing the capsule window). Also persists the hidden intent so a session
/// start will not resurrect the capsule.
fn overlay_hide<R: tauri::Runtime>(app: &AppHandle<R>) {
    if let Some(win) = app.get_webview_window(crate::windowing::OVERLAY_WINDOW_LABEL) {
        let _ = win.hide();
    }
    sync_overlay_visibility_flag(app, false);
}

/// Switch the HUD to its standby state without touching visibility: the
/// capsule is a permanent launcher now (idle -> recording -> ... -> idle), so
/// session ends return to idle instead of hiding the window.
fn overlay_emit_idle<R: tauri::Runtime>(app: &AppHandle<R>) {
    let _ = app.emit("overlay-update", serde_json::json!({ "status": "idle" }));
}

/// Show the HUD capsule in its standby state (startup, tray "show").
pub fn show_overlay_idle<R: tauri::Runtime>(app: &AppHandle<R>) {
    overlay_show(app, false);
    overlay_emit_idle(app);
}

/// Show the HUD capsule because the user explicitly asked for it from the tray
/// ("Show floating capsule"). Must always work: it overrides the persisted
/// hidden intent and re-asserts the flag.
pub fn show_overlay_forced<R: tauri::Runtime>(app: &AppHandle<R>) {
    overlay_show(app, true);
    // Only reset the capsule to standby when no session is in flight; an
    // active recording / processing state keeps its own updates flowing.
    let active = app
        .try_state::<AppState>()
        .map(|s| {
            s.session
                .lock()
                .map(|session| {
                    matches!(
                        session.status,
                        SessionStatus::Recording | SessionStatus::Processing
                    )
                })
                .unwrap_or(false)
        })
        .unwrap_or(false);
    if !active {
        overlay_emit_idle(app);
    }
}

/// Hide the HUD capsule (tray "hide floating capsule", closing the capsule
/// window). PTT keyboard and middle-click triggers are unaffected; session
/// starts simply keep the capsule hidden.
pub fn hide_overlay<R: tauri::Runtime>(app: &AppHandle<R>) {
    overlay_hide(app);
}

/// Return the HUD to standby after a short delay so the final success / error
/// state stays readable. We re-check the session status first so a newly
/// started session is never reset to idle.
///
/// `pub(crate)`: `translate.rs` reuses this instead of keeping a private copy
/// with the same semantics.
pub(crate) fn overlay_idle_later<R: tauri::Runtime>(app: &AppHandle<R>) {
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
            overlay_emit_idle(&handle);
        }
    });
}

/// Record and broadcast a session error, reset the state machine to `Idle`
/// (so it can never get stuck in `Processing`), surface it on the HUD, and
/// schedule the return to the standby capsule. Every stop failure path funnels
/// through here.
fn emit_session_error<R: tauri::Runtime>(app: &AppHandle<R>, state: &AppState, message: String) {
    // Ending the session (even by failure) invalidates middle-button
    // ownership: a later middle-button release must not try to stop anything.
    crate::mouse::clear_owned_session();
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
    overlay_idle_later(app);
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

/// "Nothing is recording" sentinel returned by [`stop_session_inner`]'s claim
/// step. The session time-limit watchdog compares against it to tell "lost the
/// race to a normal stop" (stay silent — never emit a bogus "no recording"
/// toast, mouse.rs rule) from a real failure that must reset the state machine.
const ERR_NOT_RECORDING: &str = "当前没有正在进行的录音。";

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

    // Any stop attempt (PTT release, `stop_session`, middle-button worker)
    // invalidates middle-button ownership before the session state changes.
    crate::mouse::clear_owned_session();

    {
        let mut session = state.session.lock().map_err(|e| e.to_string())?;
        if session.status != SessionStatus::Recording {
            return Err(ERR_NOT_RECORDING.into());
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
    let (wav, duration_ms) = {
        let mut session = state.session.lock().map_err(|e| e.to_string())?;
        // Real audio duration from buffered PCM samples (computed before the
        // buffer is drained by take_pcm_wav, under the same session lock).
        let sample_rate = if session.sample_rate > 0 {
            session.sample_rate as u64
        } else {
            16_000
        };
        let pcm_duration_ms = (session.pcm.len() as u64) * 1000 / sample_rate;
        let wav = session.take_pcm_wav();
        // Classic chunk fallback (webm blobs) has no reliable duration: keep None there.
        let duration_ms = if wav.len() > 44 {
            Some(pcm_duration_ms)
        } else {
            None
        };
        (wav, duration_ms)
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
                return Err(
                    "本地识别运行时尚未接入，请使用 MiMo 云端识别（设置 → 识别引擎）。".into(),
                );
            }
            _ => {
                // Default: MiMo cloud ASR. Stream the response so the HUD can
                // show the transcript as it grows (perceived latency); any
                // stream failure falls back to one non-stream retry below.
                let (api_key, base, model) = mimo_asr_credentials(&config);
                if api_key.trim().is_empty() {
                    return Err("MiMo API Key 为空，请到「设置 - 云端识别」填写。".into());
                }
                let on_partial = {
                    let app = app.clone();
                    move |partial: &str| {
                        let _ = app.emit(
                            "overlay-update",
                            serde_json::json!({
                                "status": "processing",
                                "processingStage": "transcribing",
                                "transcript": partial,
                            }),
                        );
                    }
                };
                match asr::transcribe_cloud_stream(
                    &api_key, &base, &model, &wav, "auto", on_partial,
                )
                .await
                {
                    Ok(text) => text,
                    Err(e) => {
                        log::warn!("streaming ASR failed ({e}), retrying non-stream");
                        asr::transcribe_cloud(&api_key, &base, &model, &wav, "auto").await?
                    }
                }
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
    if !final_text.trim().is_empty() && config.llm_refine.enabled {
        let _ = app.emit(
            "overlay-update",
            serde_json::json!({
                "status": "processing",
                "processingStage": "refining",
                "transcript": final_text,
            }),
        );
        match llm::refine_text(&final_text, &config.llm_refine).await {
            Ok(processed) if !processed.trim().is_empty() => final_text = processed,
            Ok(_) => {}
            Err(e) => {
                log::warn!("refine failed, using raw transcript: {e}");
            }
        }
    }

    // `translate_output` used to be OR-ed into the refine condition above while
    // only refine_text was ever called, so enabling translation alone did
    // nothing. Run the translator on its own (after refine when both are on).
    if !final_text.trim().is_empty() && config.llm_refine.translate_output {
        let _ = app.emit(
            "overlay-update",
            serde_json::json!({
                "status": "processing",
                "processingStage": "translating",
                "transcript": final_text,
            }),
        );
        match llm::translate_text(
            &final_text,
            &config.translation.target_language,
            &config.llm_refine,
        )
        .await
        {
            Ok(translated) if !translated.trim().is_empty() => final_text = translated,
            Ok(_) => {}
            Err(e) => {
                log::warn!("translate failed, using untranslated text: {e}");
            }
        }
    }

    if !final_text.trim().is_empty() {
        // Lifetime stats live in their own store (never pruned, immune to
        // history deletion). Record BEFORE `HistoryManager::add`: on first run
        // the stats backfill seeds from raw history, and if this item were
        // already in history at that point it would be counted twice.
        if let Err(e) = StatsManager::record_add(&final_text, duration_ms) {
            log::warn!("stats record failed: {e}");
        }
        let _ = HistoryManager::add(final_text.clone(), duration_ms);
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
    overlay_idle_later(app);
    Ok(final_text)
}

pub fn start_session_internal<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<Value, String> {
    let state = app.state::<AppState>();
    let config = state.config.load();
    // Prefer streaming capture so we always get PCM for cloud ASR.
    let payload = {
        let mut session = state.session.lock().map_err(|e| e.to_string())?;
        // Reject any session that is not fully settled. `Recording` is the
        // long-standing "Already recording" case; `Processing` is the critical
        // one: transcription + refine + translate can run 60s+, and starting
        // over it (`session.start(true)`) would silently orphan the in-flight
        // session and lose its transcript. This guard is the single entry
        // point for all three triggers (keyboard PTT, middle button, HUD
        // capsule pointerdown), so all of them are covered. Callers fail
        // quietly (no error toast): pressing PTT while the previous dictation
        // is still processing is simply ignored.
        match session.status {
            SessionStatus::Idle | SessionStatus::Completed | SessionStatus::Error => {}
            SessionStatus::Recording => return Err("Already recording".into()),
            SessionStatus::Processing => return Err("Processing in progress".into()),
        }
        session.start(true)
    };
    // A successful start invalidates any middle-button ownership token (the
    // mouse worker re-records its own token after this call returns).
    crate::mouse::clear_owned_session();
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
    // Rust-side backstop for the 5-minute session cap (W4): the renderer arms
    // its own `setTimeout` cap too, but a lost renderer timer (or a dead
    // renderer) must never leave a recording running forever — the mouse
    // bounce-tolerance worst case depends on a cap existing. See
    // `spawn_session_time_watchdog`.
    spawn_session_time_watchdog(app, payload.session_id.clone());
    // Show the HUD and switch it to the recording state.
    overlay_show(app, false);
    let _ = app.emit(
        "overlay-update",
        serde_json::json!({ "status": "recording" }),
    );
    serde_json::to_value(&payload).map_err(|e| e.to_string())
}

/// Rust-side watchdog for the maximum session duration ([`SESSION_MAX_DURATION_SECONDS`]).
///
/// The renderer arms a cap timer of its own (`SESSION_MAX_DURATION_SECONDS` in
/// `src/shared/constants.ts` via `setTimeout` in `AudioRecorder.tsx`) that ends
/// the session through `stopSession` — but a lost renderer timer (or a dead
/// renderer) would leave the recording running forever. This watchdog is the
/// second backstop for the same budget, so the two are dual fallbacks.
///
/// It runs the *same* stop pipeline the renderer's timer triggers
/// ([`stop_session_inner`]: transcribe -> refine/translate -> inject), not the
/// `cancel_session` path: `cancel` discards the buffered audio, so a watchdog
/// firing just before the renderer's timer would silently drop a legitimate
/// five-minute dictation that gets transcribed today. When the watchdog wins
/// the race, its `stop-recording` event makes the renderer clear its own cap
/// timer, so the two normally never both fire. Boundary: the watchdog only
/// *stops* the session; microphone teardown and UI cleanup remain the
/// renderer's job (via the `stop-recording` event) — Rust-side, the stop
/// pipeline already releases the buffered audio and invalidates the mouse
/// ownership token on its own.
fn spawn_session_time_watchdog<R: tauri::Runtime>(app: &AppHandle<R>, session_id: String) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(SESSION_MAX_DURATION_SECONDS)).await;
        if !session_past_time_limit(&handle, &session_id) {
            return;
        }
        log::warn!(
            "session {session_id} hit the {SESSION_MAX_DURATION_SECONDS}s time limit; stopping"
        );
        match stop_session_inner(&handle).await {
            Ok(_) => {}
            // A normal stop claimed the session between the check above and
            // the claim inside `stop_session_inner`: stay silent, exactly like
            // mouse.rs does for a stale middle-button release.
            Err(err) if err == ERR_NOT_RECORDING => {
                log::debug!("session watchdog: {session_id} already stopped");
            }
            // Real failure after the claim: same handling as
            // `stop_session_internal` — reset the state machine (never leave
            // status = Processing) and surface the error.
            Err(err) => {
                let state = handle.state::<AppState>();
                emit_session_error(&handle, &state, err);
            }
        }
    });
}

/// Whether the watched session is *the* session still recording and past its
/// deadline. Mirrors mouse.rs's ownership-token rule: id + status are checked
/// under the session lock, so a stopped, cancelled or replaced session is
/// never disturbed. The deadline re-check (`started_at`) additionally defeats
/// the theoretical `session-{now_ms}` id reuse if two sessions ever started in
/// the same millisecond: a fresh session is not past its own deadline.
fn session_past_time_limit<R: tauri::Runtime>(app: &AppHandle<R>, watched_id: &str) -> bool {
    let state = app.state::<AppState>();
    let session = match state.session.lock() {
        Ok(session) => session,
        Err(e) => {
            log::warn!("session time-limit check failed: {e}");
            return false;
        }
    };
    session.id == watched_id
        && session.status == SessionStatus::Recording
        && session_time_limit_reached(session.started_at, now_ms())
}

#[tauri::command]
pub fn get_config(state: State<AppState>) -> AppConfig {
    state.config.load_masked()
}

/// Result of `set_config`: the fully persisted (masked) config plus whether the
/// hotkey accelerators were rolled back because the new ones could not be
/// registered. On rollback the renderer must overwrite its hotkey state with
/// `config.hotkey` and clear the dirty flag, so the rejected value is never
/// re-sent (which would re-trigger the error toast on every autosave).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetConfigResult {
    pub config: AppConfig,
    pub hotkey_rolled_back: bool,
}

#[tauri::command]
pub fn set_config(
    app: AppHandle,
    state: State<AppState>,
    config: Value,
) -> Result<SetConfigResult, String> {
    // Keep the previously working accelerators so we can roll back if the new
    // ones cannot be registered (e.g. the OS rejects a key).
    let previous_hotkeys = state.config.load().hotkey;
    let merged = state.config.merge(&config)?;
    // Mouse middle-click push-to-talk takes effect immediately: the hook is
    // installed / removed on its own thread (mouse.rs), so this is a cheap,
    // thread-safe toggle rather than a rebind with rollback semantics.
    crate::mouse::set_middle_click_ptt(merged.mouse.middle_click_ptt);
    let mut hotkey_rolled_back = false;
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
        hotkey_rolled_back = true;
        let _ = app.emit(
            "error",
            serde_json::json!({ "message": format!("快捷键不受支持，已恢复为原设置：{err}") }),
        );
    }
    Ok(SetConfigResult {
        config: state.config.load_masked(),
        hotkey_rolled_back,
    })
}

/// Toggle the guard used by the settings hotkey recorder: while active, global
/// hotkey presses (PTT / translate) are ignored so recording a combination in
/// the settings UI cannot trigger an action. The renderer resets this on every
/// recorder exit path (commit, Esc, blur, unmount).
#[tauri::command]
pub fn set_hotkey_capture_active(state: State<AppState>, active: bool) {
    state.set_hotkey_capture_active(active);
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
pub fn get_stats() -> StatsStore {
    StatsManager::get()
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
    // Cancelling ends the session: middle-button ownership goes with it.
    crate::mouse::clear_owned_session();
    let mut session = state.session.lock().map_err(|e| e.to_string())?;
    session.cancel();
    let _ = app.emit("stop-recording", session.id.clone());
    drop(session);
    // Cancelled sessions go straight back to standby; the capsule stays up.
    overlay_emit_idle(&app);
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
    let level = entry
        .get("level")
        .and_then(|v| v.as_str())
        .unwrap_or("info");
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

/// Resize the HUD overlay capsule (logical px) and keep it anchored to its
/// persisted drag position (or bottom-centered when none is stored).
///
/// Renderer contract (`src/lib/tauri-api.ts`): invoke name `set_overlay_size`,
/// arguments `{ width, height }`. The command always targets the `overlay`
/// window regardless of which window invoked it, so a misrouted call can never
/// resize the settings window. Both values are clamped to
/// [`OVERLAY_MIN_SIZE`]..=[`OVERLAY_MAX_SIZE`] before use; no lower value is
/// rejected loudly — the caller simply gets the clamped result.
#[tauri::command]
pub fn set_overlay_size(window: WebviewWindow, width: u32, height: u32) -> Result<(), String> {
    let app = window.app_handle();
    let win = app
        .get_webview_window(crate::windowing::OVERLAY_WINDOW_LABEL)
        .ok_or_else(|| "Overlay window is unavailable".to_string())?;
    let width = width.clamp(OVERLAY_MIN_SIZE.0, OVERLAY_MAX_SIZE.0);
    let height = height.clamp(OVERLAY_MIN_SIZE.1, OVERLAY_MAX_SIZE.1);
    // Logical size: the capsule is laid out in logical px everywhere else
    // (`OverlayFrame` derives its physical size per monitor).
    win.set_size(LogicalSize::new(width as f64, height as f64))
        .map_err(|e| e.to_string())?;
    // Re-anchor after resizing so the capsule keeps the user's dragged spot
    // (or the bottom-center default) instead of springing back on resize.
    position_overlay(app, &win);
    Ok(())
}

/// Persist the HUD capsule's dragged position (or reset it to the default).
///
/// Renderer contract (`src/lib/tauri-api.ts`): invoke name
/// `set_overlay_position`, arguments `{ x, y }` in **physical pixels** taken
/// straight from the overlay window's `outerPosition()` when the drag ends.
/// Passing `null` for either coordinate is the reset path: the persisted
/// position is cleared and the capsule returns to the bottom-center default.
///
/// The dropped position is snapped to the nearest edge of the owning
/// monitor's work area ([`OVERLAY_EDGE_GAP`] physical px gap; multi-monitor
/// and DPI safe), clamped into that work area, applied to the window and
/// persisted to `app.overlayPosition`.
#[tauri::command]
pub fn set_overlay_position(
    window: WebviewWindow,
    x: Option<f64>,
    y: Option<f64>,
) -> Result<(), String> {
    let app = window.app_handle();
    let win = app
        .get_webview_window(crate::windowing::OVERLAY_WINDOW_LABEL)
        .ok_or_else(|| "Overlay window is unavailable".to_string())?;
    let state = app.state::<AppState>();
    match (x, y) {
        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => {
            let (sx, sy) = place_overlay_snapped(app, &win, x, y)
                .ok_or_else(|| "Unable to resolve the target monitor".to_string())?;
            state.config.merge(&serde_json::json!({
                "app": { "overlayPosition": { "x": sx, "y": sy } }
            }))?;
        }
        (Some(_), Some(_)) => return Err("Invalid overlay position".into()),
        // Reset: clear the persisted position and fall back to the default.
        _ => {
            state.config.merge(&serde_json::json!({
                "app": { "overlayPosition": null }
            }))?;
            position_overlay_bottom_center(app, &win);
        }
    }
    Ok(())
}

/// Show the main (settings) window, rebuilding it on demand if the delayed
/// destroy already reclaimed it (`windowing::ensure_main_window`).
///
/// `async` on purpose: on Windows, creating a webview window from a
/// synchronous IPC handler deadlocks (WebView2 reentrancy, wry#583), so the
/// window work runs on the async runtime / a worker thread instead.
#[tauri::command]
pub async fn show_settings_window(app: AppHandle) -> Result<(), String> {
    crate::windowing::open_main_window(&app);
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
