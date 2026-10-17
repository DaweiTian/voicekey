use crate::asr;
use crate::audio::{
    now_ms, session_time_limit_reached, AudioChunkPayload, SessionStatus, StreamingAudioEndPayload,
    StreamingAudioFramePayload, SESSION_MAX_DURATION_SECONDS,
};
use crate::config::{AppConfig, ConfigSecretRequest};
use crate::filler;
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
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Monitor, PhysicalPosition, State,
    WebviewWindow,
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

/// Gap between the HUD window frame and the work-area edge it snaps to in the
/// legacy window-frame snap (renderer reports no capsule size), in physical
/// pixels (the contract unit for drag positions).
const OVERLAY_EDGE_GAP: f64 = 10.0;

/// Magnetic edge-snap threshold (`set_overlay_position`, physical px): a
/// dropped capsule snaps to a work-area edge when its *visible* edge is within
/// this distance of it **or has already crossed it**. The distance is signed
/// (see [`snap_overlay_position`]): positive = gap inside the work area, zero =
/// touching, negative = the capsule sticks out past the edge. Every negative
/// value qualifies however deep the overflow — 0.5.7 regression fix: dragging
/// the capsule *past* the edge ("拖过头") is a stronger edge signal than a
/// touch and must dock, not get rejected by a `0..=48` window. Only a drop
/// still clear of every edge by more than the threshold is respected as-is
/// (clamped into the work area) instead of flying to the nearest edge. Only
/// the capsule-frame snap uses the threshold; the legacy window-frame fallback
/// snaps unconditionally, exactly like pre-capsule builds.
const OVERLAY_SNAP_THRESHOLD: f64 = 48.0;

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

    /// Clamp a physical position so the capsule's **center line** stays inside
    /// the work area. The capsule is centered in the fixed-size window, so its
    /// center line *is* the window's center line — no capsule size needed.
    ///
    /// This is the clamp for edge-snapped positions and for every show /
    /// re-anchor path: a center exactly ON the work-area edge is legal — that
    /// is the half-hidden drawer dock (half the capsule embedded in the screen
    /// edge) — so re-clamping never pulls a docked position back, while a
    /// wayward center (monitor layout changed) is pulled to the nearest legal
    /// point and the capsule can never be fully hidden.
    fn clamp_center(&self, x: f64, y: f64) -> (f64, f64) {
        (
            x.clamp(
                self.area_x - self.window_w / 2.0,
                self.area_x + self.area_w - self.window_w / 2.0,
            ),
            y.clamp(
                self.area_y - self.window_h / 2.0,
                self.area_y + self.area_h - self.window_h / 2.0,
            ),
        )
    }

    /// Legacy clamp (pre-capsule window-frame snapping and the bottom-center
    /// default): keep the whole window inside the work area, so the default /
    /// reset placement is always fully visible. An oversized window degrades
    /// to anchored at the area origin.
    fn clamp_window(&self, x: f64, y: f64) -> (f64, f64) {
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
    ///
    /// Skipped when the window is already there (sub-pixel slack: the
    /// logical round-trip can round by <1 physical px). A same-position
    /// `set_position` is visually a no-op but still enters the platform window
    /// proc, and `start_session_internal` funnels every session start through
    /// here while a pointer gesture may still be live (PTT hold / latch tap) —
    /// 0.5.3 taught that any geometry call during a gesture is a theoretical
    /// pointercancel insertion point, so it must not fire needlessly.
    fn apply<R: tauri::Runtime>(&self, win: &WebviewWindow<R>, x: f64, y: f64) {
        if let Ok(pos) = win.outer_position() {
            if (pos.x as f64 - x).abs() < 1.0 && (pos.y as f64 - y).abs() < 1.0 {
                return;
            }
        }
        let _ = win.set_position(LogicalPosition::new(
            x / self.window_scale,
            y / self.window_scale,
        ));
    }
}

/// Pure bottom-center placement inside a work area (all physical px): the
/// window's horizontal center on the area's center, its bottom edge
/// [`OVERLAY_BOTTOM_MARGIN`]-scaled above the area's bottom. The margin is
/// handed in pre-scaled so the function stays free of monitor context.
///
/// Shared by the overlay's default / reset placement
/// ([`position_overlay_bottom_center`]) and the session capsule's per-start
/// placement ([`position_session_bottom_center`]): both stand for "the fixed
/// bottom-center capsule spot" — keep them in step.
fn bottom_center_in(area: (f64, f64, f64, f64), window: (f64, f64), margin: f64) -> (f64, f64) {
    let (area_x, area_y, area_w, area_h) = area;
    // Horizontal: work-area center.
    let x = area_x + (area_w - window.0) / 2.0;
    // Vertical: `margin` above the work-area bottom (taskbar).
    let y = area_y + area_h - window.1 - margin;
    (x, y)
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

    let (x, y) = bottom_center_in(
        (frame.area_x, frame.area_y, frame.area_w, frame.area_h),
        (frame.window_w, frame.window_h),
        margin,
    );

    // The default / reset placement is always fully visible (double-click
    // reset lands here, never through the snap path).
    let (x, y) = frame.clamp_window(x, y);
    frame.apply(win, x, y);
}

/// Place the session capsule at the fixed bottom-center spot of the monitor
/// that contains the cursor **at session start** (multi-monitor: the cursor's
/// monitor at that instant decides where the capsule stands). Unlike the
/// overlay, the session window has no persisted position and is never dragged:
/// it is positioned once per show and hidden again at the end of the session.
///
/// INVARIANT: this only ever touches the *session* window's geometry — the
/// overlay window (possibly mid-gesture under the user's finger) is never
/// repositioned or resized by a session start.
fn position_session_bottom_center<R: tauri::Runtime>(app: &AppHandle<R>, win: &WebviewWindow<R>) {
    let Ok(cursor) = app.cursor_position() else {
        return;
    };
    let Some(frame) = OverlayFrame::resolve(app, win, cursor.x, cursor.y) else {
        return;
    };
    let margin = OVERLAY_BOTTOM_MARGIN * frame.monitor_scale;
    let (x, y) = bottom_center_in(
        (frame.area_x, frame.area_y, frame.area_w, frame.area_h),
        (frame.window_w, frame.window_h),
        margin,
    );
    let (x, y) = frame.clamp_window(x, y);
    frame.apply(win, x, y);
}

/// Place the HUD at its persisted drag position. The position is re-clamped
/// into the work area of the monitor that owns it (monitor layouts change; the
/// capsule must never end up fully hidden) but never re-snapped or re-centered:
/// this is what keeps "drag away, then resize / show" from springing back to
/// the bottom-center default. The clamp is [`OverlayFrame::clamp_center`], so
/// a half-hidden drawer dock (capsule center line on the work-area edge)
/// survives re-clamping unchanged.
fn position_overlay_persisted<R: tauri::Runtime>(
    app: &AppHandle<R>,
    win: &WebviewWindow<R>,
    x: f64,
    y: f64,
) {
    let Some(frame) = OverlayFrame::resolve(app, win, x, y) else {
        return;
    };
    let (x, y) = frame.clamp_center(x, y);
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

/// Extract the renderer-reported capsule size (**logical** px, measured from
/// the DOM as `offsetWidth` / `offsetHeight`) from the optional `capsule`
/// invoke argument. Anything malformed (missing keys, non-numeric, non-finite)
/// is ignored so the caller falls back to the legacy window-frame snap — a
/// hostile renderer can never panic or hijack the geometry.
fn capsule_dims(capsule: &Option<serde_json::Value>) -> Option<(f64, f64)> {
    let obj = capsule.as_ref()?.as_object()?;
    let width = obj.get("width")?.as_f64()?;
    let height = obj.get("height")?.as_f64()?;
    if width.is_finite() && height.is_finite() {
        Some((width, height))
    } else {
        None
    }
}

/// Whether a physical capsule size can define a visible frame at all: finite,
/// positive, and able to fit inside the window. Anything else is malformed
/// renderer input and falls back to the legacy window-frame behavior.
fn capsule_fits(window: (f64, f64), capsule: (f64, f64)) -> bool {
    capsule.0.is_finite()
        && capsule.1.is_finite()
        && capsule.0 > 0.0
        && capsule.1 > 0.0
        && capsule.0 <= window.0
        && capsule.1 <= window.1
}

/// A snap decision that actually docked: where to move the window and which
/// work-area edge it docked to.
///
/// The `edge` is what [`set_overlay_position`] reports back to the renderer
/// (`Some(edge)` = docked → compact idle capsule; `None` = the drop only got
/// clamped → expanded capsule).
#[derive(Debug, Clone, Copy, PartialEq)]
struct SnapLanding {
    /// Snapped window position, physical px.
    pos: (f64, f64),
    /// Work-area edge the landing hugs: `"left" | "right" | "top" | "bottom"`.
    edge: &'static str,
}

/// Pure snap decision for a dropped overlay position (all values physical px):
/// picks the work-area edge and computes the snapped window position, or
/// `None` when the drop should not snap at all.
///
/// - `window`: the overlay window's physical size (the capsule is centered in
///   it, so the centering offset between the two frames is derived here).
/// - `capsule`: the visible capsule's physical size, `None` = legacy
///   window-frame behavior (also the fallback for malformed sizes, see
///   [`capsule_fits`]).
/// - `area`: work area (x, y, w, h); `x, y`: dropped window position.
///
/// Capsule-frame snap (the default since the capsule is what the user sees):
/// the *capsule's* distance to each work-area edge picks the edge, and the
/// landing is the **half-hidden drawer** position: the capsule's **center
/// line** lands ON the work-area edge — half the capsule inside the screen,
/// half outside. Per edge (physical px):
/// `windowX = workLeft - capsuleOffsetX - capsuleW/2`,
/// `windowX = workRight - capsuleOffsetX - capsuleW/2`,
/// `windowY = workTop - capsuleOffsetY - capsuleH/2`,
/// `windowY = workBottom - capsuleOffsetY - capsuleH/2`.
/// Since `capsuleOffset + capsule/2 = window/2`, the docked window position is
/// the same for every capsule size: the standby and session capsules dock on
/// the same center line and never jump when they morph.
/// The edge test uses **signed** distances (positive = gap inside the work
/// area, zero = touching, negative = the capsule already sticks out past the
/// edge) and the winning edge is the *smallest* signed distance — the closest
/// touch or the deepest overflow. Any distance at or below
/// [`OVERLAY_SNAP_THRESHOLD`] snaps, so touching and overflowing both dock
/// (overflow is the stronger signal, 0.5.7); only a drop more than the
/// threshold clear of every edge
/// returns `None` (keep the drop, just clamp it). The decision is made on the
/// raw dropped coordinates — clamping happens afterwards and never feeds back
/// into it.
///
/// Legacy window-frame snap (`capsule: None`): the dropped *point* picks the
/// nearest edge and the *window* edge lands [`OVERLAY_EDGE_GAP`] from it,
/// unconditionally — exactly the historical behavior.
fn snap_overlay_position(
    window: (f64, f64),
    capsule: Option<(f64, f64)>,
    area: (f64, f64, f64, f64),
    x: f64,
    y: f64,
) -> Option<SnapLanding> {
    let (area_x, area_y, area_w, area_h) = area;
    let capsule = capsule.filter(|(w, h)| capsule_fits(window, (*w, *h)));
    let Some((capsule_w, capsule_h)) = capsule else {
        // Legacy window-frame behavior: nearest work-area edge to the dropped
        // point wins and the window edge lands OVERLAY_EDGE_GAP from it.
        let d_left = (x - area_x).abs();
        let d_right = (area_x + area_w - x).abs();
        let d_top = (y - area_y).abs();
        let d_bottom = (area_y + area_h - y).abs();
        let nearest = d_left.min(d_right).min(d_top).min(d_bottom);
        let (mut nx, mut ny, mut edge) = (x, y, "bottom");
        if nearest == d_left {
            nx = area_x + OVERLAY_EDGE_GAP;
            edge = "left";
        } else if nearest == d_right {
            nx = area_x + area_w - window.0 - OVERLAY_EDGE_GAP;
            edge = "right";
        } else if nearest == d_top {
            ny = area_y + OVERLAY_EDGE_GAP;
            edge = "top";
        } else {
            ny = area_y + area_h - window.1 - OVERLAY_EDGE_GAP;
        }
        return Some(SnapLanding {
            pos: (nx, ny),
            edge,
        });
    };

    // Capsule-frame snap: the capsule is centered in the window, so its offset
    // from each window edge is fixed for a given size.
    let offset_x = (window.0 - capsule_w) / 2.0;
    let offset_y = (window.1 - capsule_h) / 2.0;
    let capsule_left = x + offset_x;
    let capsule_top = y + offset_y;
    // Signed distance from each capsule side to its work-area edge: positive =
    // stops short of the edge (gap), zero = touching, negative = the capsule
    // sticks out past it. Distances MUST stay signed (0.5.7 regression fix):
    // `abs()` used to fold a deep overflow back into the `0..=48` window from
    // the other side, so a capsule dragged half past the edge (e.g. 52 px out)
    // read as "52 px away" and was rejected — "拖过头反而不吸".
    let d_left = capsule_left - area_x;
    let d_right = area_x + area_w - (capsule_left + capsule_w);
    let d_top = capsule_top - area_y;
    let d_bottom = area_y + area_h - (capsule_top + capsule_h);
    // Smallest signed distance = strongest edge signal (closest touch or
    // deepest overflow) and picks the edge; exact ties keep the historical
    // priority left > right > top > bottom (the if-chain order below).
    let nearest = d_left.min(d_right).min(d_top).min(d_bottom);
    // Magnetic threshold: snap when any side is within OVERLAY_SNAP_THRESHOLD
    // of its edge **or has crossed it** (every negative value qualifies, no
    // matter how deep the overflow). Only a drop still clear of every edge by
    // more than the threshold stays where it landed (the caller clamps it into
    // the work area and persists). The decision runs on the raw dropped
    // coordinates — never on a clamped position.
    if nearest > OVERLAY_SNAP_THRESHOLD {
        return None;
    }
    // Half-hidden drawer landing: the capsule's center line lands ON the
    // winning work-area edge — `window = workEdge - offset - capsule/2`
    // (half the capsule docked into the screen edge, half left visible).
    let (mut nx, mut ny, mut edge) = (x, y, "bottom");
    if nearest == d_left {
        nx = area_x - offset_x - capsule_w / 2.0;
        edge = "left";
    } else if nearest == d_right {
        nx = area_x + area_w - offset_x - capsule_w / 2.0;
        edge = "right";
    } else if nearest == d_top {
        ny = area_y - offset_y - capsule_h / 2.0;
        edge = "top";
    } else {
        ny = area_y + area_h - offset_y - capsule_h / 2.0;
    }
    Some(SnapLanding {
        pos: (nx, ny),
        edge,
    })
}

/// Outcome of a successful drop: the position applied to the window plus
/// whether it **docked** to a work-area edge.
struct PlacedOverlay {
    /// Position applied (and persisted), physical px.
    pos: (f64, f64),
    /// `Some(edge)` when the drop snapped to that edge (docked → renderer
    /// shows the compact idle capsule); `None` when it only got clamped.
    edge: Option<&'static str>,
}

/// Snap a dropped physical position to the nearest edge of the owning
/// monitor's work area and move the window there (returns the position for
/// persistence).
///
/// `capsule` is the renderer-measured capsule size in **logical** px (`None`
/// when the renderer sent nothing usable): with it, snapping happens in the
/// capsule's frame — the *visible capsule* docks **half-hidden** with its
/// center line on the work-area edge (drawer style), not the 280×64 window
/// frame around it. A drop more than [`OVERLAY_SNAP_THRESHOLD`] clear of
/// every edge is not snapped: it is only clamped and persisted as-is. Touching
/// or crossing an edge always docks (the distance test is signed — see
/// [`snap_overlay_position`]); the snap decision runs on the raw dropped
/// coordinates and the snapped landing overrides them, so a clamp can never
/// pull an overflowed drop back inside before the edge is judged.
fn place_overlay_snapped<R: tauri::Runtime>(
    app: &AppHandle<R>,
    win: &WebviewWindow<R>,
    x: f64,
    y: f64,
    capsule: Option<(f64, f64)>,
) -> Option<PlacedOverlay> {
    let frame = OverlayFrame::resolve(app, win, x, y)?;
    // Renderer capsule dims are logical px (DOM `offsetWidth`/`offsetHeight`);
    // the snap math is physical px, converted with the same monitor scale
    // `OverlayFrame` uses for the window size. Malformed sizes (see
    // `capsule_fits`) fall back to the legacy window-frame snap.
    let window = (frame.window_w, frame.window_h);
    let capsule_px = capsule
        .map(|(w, h)| (w * frame.monitor_scale, h * frame.monitor_scale))
        .filter(|(w, h)| capsule_fits(window, (*w, *h)));
    let area = (frame.area_x, frame.area_y, frame.area_w, frame.area_h);
    let landing = snap_overlay_position(window, capsule_px, area, x, y);
    let (tx, ty, edge) = match landing {
        Some(landing) => (landing.pos.0, landing.pos.1, Some(landing.edge)),
        None => (x, y, None),
    };
    // Capsule frame: clamp by the capsule center so a half-hidden dock (center
    // line on the work-area edge) is legal and survives untouched. Legacy
    // fallback keeps the historical whole-window clamp.
    let (sx, sy) = if capsule_px.is_some() {
        frame.clamp_center(tx, ty)
    } else {
        frame.clamp_window(tx, ty)
    };
    frame.apply(win, sx, sy);
    Some(PlacedOverlay {
        pos: (sx, sy),
        edge,
    })
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
///
/// **0.5.7 white-flash fix:** when the capsule is *already* on screen - the
/// normal case, since the standby capsule is shown once at startup and stays up,
/// and every session start calls this path again through
/// `start_session_internal` - the `show()` + ghost-titlebar nudge below are
/// skipped. `show()` on an already visible window buys nothing, and the nudge is
/// a **1px bounds change on a visible transparent window**, which is precisely
/// what makes WebView2/DWM present a frame before the page content for the new
/// bounds exists: the white flash reported after 0.5.6 (the nudge is the only
/// geometry mutation 0.5.6 added on the click/start path). The nudge is only
/// meaningful around a *visibility change*, because that is what re-creates the
/// WebView2 surface and re-triggers the ghost titlebar (tauri#14764). The
/// Windows hardening is skipped here too: it was applied when the window was
/// shown, it is idempotent, and `lib.rs`'s `Focused` handler re-asserts it on
/// every activation - the only event that redraws the DWM focus border.
fn show_overlay_window<R: tauri::Runtime>(app: &AppHandle<R>, win: &WebviewWindow<R>) {
    let already_visible = win.is_visible().unwrap_or(false);
    if already_visible {
        // Keep the capsule anchored at its persisted spot (a no-op while the
        // coordinates are unchanged) and leave its surface completely alone.
        crate::windowing::trace(
            "overlay show: already visible -> position+flag only (no show/harden/nudge)",
        );
        position_overlay(app, win);
        sync_overlay_visibility_flag(app, true);
        return;
    }
    crate::windowing::trace("overlay show: hidden -> harden+position+show+harden+nudge");
    crate::windowing::harden_overlay_window(win);
    position_overlay(app, win);
    if win.show().is_ok() {
        crate::windowing::harden_overlay_window(win);
        // WebView2 runtime 144+ repaints a stale "ghost titlebar" carrying the
        // page title over the transparent capsule on any activation
        // (tauri#14764); a resize clears it, so nudge once here as well as on
        // every focus change (lib.rs). Guarded and idempotent.
        crate::windowing::clear_webview_ghost_titlebar(win);
        sync_overlay_visibility_flag(app, true);
    }
}

/// Hide the HUD overlay window immediately (tray "hide floating capsule" and
/// closing the capsule window). Also persists the hidden intent so a session
/// start will not resurrect the capsule.
///
/// INVARIANT: this only ever touches the `overlay` window — the session
/// capsule (fixed bottom-center) is deliberately **not** hidden here: a tray
/// toggle of the standby capsule must never take the session capsule down.
fn overlay_hide<R: tauri::Runtime>(app: &AppHandle<R>) {
    if let Some(win) = app.get_webview_window(crate::windowing::OVERLAY_WINDOW_LABEL) {
        let _ = win.hide();
    }
    sync_overlay_visibility_flag(app, false);
}

/// Position (once, at the fixed bottom-center spot of the cursor's monitor)
/// and show one existing session capsule window, re-applying the Windows
/// hardening around the show exactly like [`show_overlay_window`] does.
///
/// **0.5.7 white-flash fix - the order matters.** The ghost-titlebar nudge
/// ([`crate::windowing::clear_webview_ghost_titlebar`]) is a *bounds change*, and
/// a bounds change on a **visible** transparent window can make WebView2/DWM
/// present a frame from underneath the page (WebView2Feedback#5607: a white flash
/// "when transforming the bounds of the WebView") - the flash the user reported
/// after 0.5.6. So the nudge now runs **before `show()`**, while the window is
/// still hidden (and, because of [`hide_session`], still parked off-screen): the
/// repaint it forces cannot be seen, and the window is shown already clean.
///
/// The old post-`show()` nudge had no visible benefit to lose: the session
/// capsule is `WS_EX_NOACTIVATE` (it never activates) and is never dragged, so
/// neither of the ghost titlebar's triggers (drag / focus change, tauri#14764)
/// applies to it once it is on screen - and the 0.5.6 field test reported only a
/// background flash on this window, never a bar. Order is geometry-safe:
/// `set_size` keeps the window's position (tao uses `SWP_NOMOVE`), so anchoring
/// first and nudging second cannot move the capsule.
fn show_session_window<R: tauri::Runtime>(app: &AppHandle<R>, win: &WebviewWindow<R>) {
    crate::windowing::trace("session show: harden -> position -> nudge(hidden) -> show -> harden");
    crate::windowing::harden_overlay_window(win);
    // 1) anchor at the target spot, 2) clear the stale WebView2 layer while the
    // window is hidden (invisible repaint), 3) show the already-clean window.
    position_session_bottom_center(app, win);
    crate::windowing::clear_webview_ghost_titlebar(win);
    if win.show().is_ok() {
        crate::windowing::harden_overlay_window(win);
    }
}

/// Show the session capsule window, unconditionally and independently of the
/// overlay's visibility: the tray "hide floating capsule" toggle must never
/// suppress it (hard invariant: PTT hotkey / middle-click with the overlay
/// hidden still shows the bottom-center capsule). A destroyed window is
/// rebuilt on a worker thread (wry#583: never create a webview window while
/// blocking the main thread), mirroring [`overlay_show`].
///
/// Callers: `start_session_internal` (every session start, whatever the
/// trigger) and translate feedback ([`session_show_for_feedback`]).
fn session_show<R: tauri::Runtime>(app: &AppHandle<R>) {
    if let Some(win) = app.get_webview_window(crate::windowing::SESSION_WINDOW_LABEL) {
        show_session_window(app, &win);
    } else {
        let handle = app.clone();
        std::thread::spawn(
            move || match crate::windowing::ensure_session_window(&handle) {
                Some(win) => show_session_window(&handle, &win),
                None => log::error!("session_show: session window is unavailable"),
            },
        );
    }
}

/// Off-screen "park" position for the session capsule while it is hidden.
///
/// Why this exists (0.5.7 white-flash fix): hiding/showing a **transparent**
/// WebView2 window can present a frame from the layer underneath the page
/// (WebView2's white default background / the window's own surface) before the
/// content for the new state exists - tauri#14515, "Creating a hidden transparent
/// window results in a white flash when displayed". The reporter's verified
/// workaround is to park the window **outside every monitor before hiding it**,
/// so whatever frame is presented is physically invisible.
///
/// The park point is derived from the real monitor layout (the union of every
/// monitor, pushed past its bottom-right corner) instead of the issue's
/// hard-coded `-9999, -9999`: a wide multi-monitor wall can extend past `-9999`
/// and would drag a hard-coded park point back onto a screen. Falls back to the
/// historical constant only when the monitor list is unavailable.
fn session_park_position<R: tauri::Runtime>(app: &AppHandle<R>) -> PhysicalPosition<i32> {
    let monitors = app.available_monitors().unwrap_or_default();
    let (mut right, mut bottom) = (i32::MIN, i32::MIN);
    for monitor in &monitors {
        let pos = monitor.position();
        let size = monitor.size();
        right = right.max(pos.x.saturating_add(size.width as i32));
        bottom = bottom.max(pos.y.saturating_add(size.height as i32));
    }
    if right == i32::MIN || bottom == i32::MIN {
        // No monitor information: fall back to the documented constant.
        return PhysicalPosition::new(-9999, -9999);
    }
    // 1000 physical px past the virtual desktop's bottom-right corner: far
    // outside every work area, and still a valid i32.
    PhysicalPosition::new(right.saturating_add(1000), bottom.saturating_add(1000))
}

/// Hide the session capsule window (session ended / returned to standby).
///
/// **0.5.7 white-flash fix:** the window is parked outside every monitor
/// ([`session_park_position`]) *before* it is hidden, so the frame that `hide()`
/// can present on a transparent window is physically invisible (tauri#14515's
/// reporter: "there was still a white screen when the window was hidden. To fix
/// that, I first moved the window off-screen ... this eliminated the hidden white
/// screen"). It also means the next `session_show` starts from an off-screen
/// position; [`show_session_window`] always re-anchors before showing.
///
/// Unlike [`overlay_hide`] this never touches the overlay or its persisted
/// visibility flag — the two windows are fully decoupled. Public for the
/// `CloseRequested` handlers in `lib.rs` / `windowing.rs`, which convert a
/// renderer close into a hide.
pub fn hide_session<R: tauri::Runtime>(app: &AppHandle<R>) {
    if let Some(win) = app.get_webview_window(crate::windowing::SESSION_WINDOW_LABEL) {
        let park = session_park_position(app);
        crate::windowing::trace(&format!(
            "session hide: park=({},{}) then hide",
            park.x, park.y
        ));
        let _ = win.set_position(park);
        let _ = win.hide();
    }
}

/// Show the session capsule for non-session feedback (`translate.rs`: the
/// selected-text translation success / error flash). Shares the capsule's
/// bottom-center visuals so the feedback appears even when the overlay
/// capsule is hidden from the tray. No-op while a voice session owns the
/// capsule — its geometry must not be re-anchored mid-session (0.5.4
/// geometry invariant), and it already shows the session's own state.
pub(crate) fn session_show_for_feedback<R: tauri::Runtime>(app: &AppHandle<R>) {
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
        session_show(app);
    }
}

/// Switch the HUD to its standby state without touching visibility: the
/// capsule is a permanent launcher now (idle -> recording -> ... -> idle), so
/// session ends return to idle instead of hiding the window.
///
/// This is also the **single funnel for "session capsule down"**: every path
/// back to standby (session success / error display timeout, cancel, quiet
/// mis-tap, tray forced-show while idle, translate feedback timeout, startup)
/// lands here, so hiding the session window in exactly this one place keeps
/// "session capsule visible ⇔ a session (or feedback flash) is in flight".
fn overlay_emit_idle<R: tauri::Runtime>(app: &AppHandle<R>) {
    hide_session(app);
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

/// Below this captured-session length (ms), a session that recorded no audio at
/// all is treated as a mis-tap / quick click and settles silently — no error
/// toast, empty transcript, straight back to idle (product decision: 误触/快速
/// 点击静默不报错).
///
/// Why 250ms: `getUserMedia` typically needs ~100-200ms to hand over the first
/// audio frames, so a press-release shorter than that can reach the stop path
/// before anything was ever capturable — that is not a microphone failure and
/// must not alarm the user. A session that long (or longer) with zero samples
/// means the mic really produced nothing, so the original "没有录到声音" error
/// stays.
const EMPTY_SESSION_QUIET_MS: u64 = 250;

/// Pure decision for a session that ended with no audio: whether it settles
/// quietly (empty transcript, back to idle) instead of reporting
/// "没有录到声音，请检查麦克风后重试。". `has_audio` is true when any PCM or
/// classic chunk was captured.
///
/// Product decision (0.5.x): a mis-tap or very quick click runs the full
/// start → stop chain before the microphone stream is ready; that must be
/// silent, while a real hold ([`EMPTY_SESSION_QUIET_MS`] or longer) that
/// captured nothing keeps the microphone-failure error.
fn empty_session_is_quiet(elapsed_ms: u64, has_audio: bool) -> bool {
    !has_audio && elapsed_ms < EMPTY_SESSION_QUIET_MS
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

    // Any stop attempt (PTT release, `stop_session`, middle-button worker)
    // invalidates middle-button ownership before the session state changes.
    crate::mouse::clear_owned_session();

    // Session length at the moment the stop is claimed, before `wait_for_flush`
    // can add up to 1200ms: the empty-session quiet decision must see the real
    // press → release duration, not duration + flush latency. A clock that
    // moved backwards reads as "just started" (saturating_sub), which at worst
    // turns a rare error toast into silence — the product decision's bias.
    let session_elapsed_ms = {
        let mut session = state.session.lock().map_err(|e| e.to_string())?;
        if session.status != SessionStatus::Recording {
            return Err(ERR_NOT_RECORDING.into());
        }
        session.status = SessionStatus::Processing;
        now_ms().saturating_sub(session.started_at)
    };

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
            // Mis-tap / quick click: session ended before the mic produced
            // anything (`has_audio` false by construction here — PCM was empty
            // and no classic chunks arrived). Per product decision such short
            // sessions settle silently back to idle: complete with an empty
            // transcript, skip the `error` / `transcription` events (a toast
            // would defeat "不弹错误") and the success flash alike.
            if empty_session_is_quiet(session_elapsed_ms, false) {
                {
                    let mut session = state.session.lock().map_err(|e| e.to_string())?;
                    session.complete(String::new());
                }
                overlay_emit_idle(app);
                return Ok(String::new());
            }
            // A real hold that captured nothing is a microphone failure: keep
            // the original error so the user can act on it.
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

    // 规则档口水词清理作用在 ASR 直出文本上，先于 refine/translate，
    // 因此在润色关闭时也生效（兜底）。`llm` 档 + 润色开启时此处原样
    // 透传，由精炼 prompt 处理（仍是单次调用）。流式 partial/HUD 路径
    // 不经过这里，不受影响。
    let mut final_text = filler::cleanup_stage(
        &raw_text,
        &config.llm_refine.filler_cleanup,
        config.llm_refine.enabled,
    );

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
    crate::windowing::trace("session start: entry (hotkey / middle-click / HUD tap)");
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
    // Show the bottom-center session capsule, unconditionally and
    // independently of the overlay's visibility (tray-hidden overlay must not
    // suppress it). Positioned once, at session start, on the cursor's
    // monitor — it never moves during the session (0.5.4 geometry invariant)
    // and never touches the overlay window's geometry (a hold gesture may be
    // live under the user's finger right now).
    session_show(app);
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

/// Config patch for a real drop: the landed position **and** the docked edge
/// written together, so a restart restores both where the capsule sits and
/// which shape (compact / expanded) the renderer draws there.
fn overlay_position_patch(pos: (f64, f64), edge: Option<&'static str>) -> serde_json::Value {
    serde_json::json!({
        "app": {
            "overlayPosition": { "x": pos.0, "y": pos.1 },
            "overlayDocked": edge,
        }
    })
}

/// Config patch for the reset path (double-click / `null` coordinate): both
/// the persisted position and the dock state are cleared.
fn overlay_reset_patch() -> serde_json::Value {
    serde_json::json!({
        "app": { "overlayPosition": null, "overlayDocked": null }
    })
}

/// Persist the HUD capsule's dragged position (or reset it to the default).
///
/// Renderer contract (`src/lib/tauri-api.ts`): invoke name
/// `set_overlay_position`, arguments `{ x, y }` in **physical pixels** taken
/// straight from the overlay window's `outerPosition()` when the drag ends,
/// plus an optional `capsule: { width, height }` in **logical** px — the
/// visible capsule's measured DOM size. Passing `null` for either coordinate
/// is the reset path: the persisted position is cleared and the capsule
/// returns to the bottom-center default.
///
/// **Return value** (`Result<Option<String>, String>` → JSON `string | null`,
/// backwards compatible: older callers ignore it): `Some(edge)` with
/// `"left" | "right" | "top" | "bottom"` when the drop snapped to that
/// work-area edge (docked), `None` when the drop only got clamped (more than
/// [`OVERLAY_SNAP_THRESHOLD`] clear of every edge) **and** for the reset path. The renderer maps
/// it onto the compact idle capsule: docked → icon only, otherwise the full
/// "hold to talk" capsule. The same decision is persisted as
/// `app.overlayDocked` (see [`overlay_position_patch`]) so the compact shape
/// survives a restart.
///
/// The dropped position is snapped to the nearest edge of the owning
/// monitor's work area, clamped into that work area, applied to the window and
/// persisted to `app.overlayPosition`. With `capsule`, snapping docks the
/// capsule **half-hidden** (drawer style: its center line lands on the
/// work-area edge — half embedded in the screen edge, half visible; see
/// [`snap_overlay_position`]); without it (legacy / malformed) the window
/// frame snaps [`OVERLAY_EDGE_GAP`] from the edge as before. A drop whose
/// capsule is more than [`OVERLAY_SNAP_THRESHOLD`] clear of every edge is not
/// snapped — only clamped (by the capsule center, so a half-hidden position is
/// legal) and persisted where it landed; touching or crossing an edge always
/// docks (signed distance test, see [`snap_overlay_position`]).
#[tauri::command]
pub fn set_overlay_position(
    window: WebviewWindow,
    x: Option<f64>,
    y: Option<f64>,
    capsule: Option<serde_json::Value>,
) -> Result<Option<String>, String> {
    let app = window.app_handle();
    let win = app
        .get_webview_window(crate::windowing::OVERLAY_WINDOW_LABEL)
        .ok_or_else(|| "Overlay window is unavailable".to_string())?;
    let state = app.state::<AppState>();
    // Malformed capsule data is silently ignored (legacy window-frame snap);
    // it must never fail the command or panic.
    let capsule = capsule_dims(&capsule);
    let docked = match (x, y) {
        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => {
            let placed = place_overlay_snapped(app, &win, x, y, capsule)
                .ok_or_else(|| "Unable to resolve the target monitor".to_string())?;
            state
                .config
                .merge(&overlay_position_patch(placed.pos, placed.edge))?;
            placed.edge
        }
        (Some(_), Some(_)) => return Err("Invalid overlay position".into()),
        // Reset: clear the persisted position (and dock state) and fall back
        // to the default — the reset path never docks, so it always reports
        // `None` (expanded standby capsule).
        _ => {
            state.config.merge(&overlay_reset_patch())?;
            position_overlay_bottom_center(app, &win);
            None
        }
    };
    Ok(docked.map(str::to_string))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Mis-tap / quick click: shorter than `EMPTY_SESSION_QUIET_MS` with
    /// nothing captured settles quietly (product decision: never an error).
    #[test]
    fn empty_session_short_without_audio_is_quiet() {
        assert!(empty_session_is_quiet(0, false));
        assert!(empty_session_is_quiet(EMPTY_SESSION_QUIET_MS - 1, false));
    }

    /// A real hold that captured nothing is a microphone failure: the original
    /// "没有录到声音" error must stay.
    #[test]
    fn empty_session_long_without_audio_reports_error() {
        assert!(!empty_session_is_quiet(EMPTY_SESSION_QUIET_MS, false));
        assert!(!empty_session_is_quiet(5_000, false));
    }

    /// Any captured audio follows the normal transcription path regardless of
    /// duration — a short utterance is still a real dictation.
    #[test]
    fn session_with_audio_is_never_quieted_by_duration() {
        assert!(!empty_session_is_quiet(0, true));
        assert!(!empty_session_is_quiet(5_000, true));
    }

    // ---- session / overlay capsule placement -------------------------------

    /// The fixed bottom-center spot (overlay default placement + session
    /// capsule per-start placement): work-area horizontal center, `margin`
    /// above the work-area bottom. Pure layer behind
    /// `position_overlay_bottom_center` / `position_session_bottom_center`.
    #[test]
    fn bottom_center_in_places_window_bottom_centered() {
        // Work area (100, 50, 1920, 1040), window 280×64, margin 48.
        let (x, y) = bottom_center_in(AREA, WINDOW, 48.0);
        assert_eq!(x, AREA.0 + (AREA.2 - WINDOW.0) / 2.0);
        assert_eq!(y, AREA.1 + AREA.3 - WINDOW.1 - 48.0);
        // Fully inside the work area (a visible placement, like clamp_window
        // guarantees after clamping).
        assert!(x >= AREA.0 && x + WINDOW.0 <= AREA.0 + AREA.2);
        assert!(y >= AREA.1 && y + WINDOW.1 <= AREA.1 + AREA.3);
    }

    /// Same math on a second-monitor work area with a non-zero origin and a
    /// DPI-scaled margin: only the margin scales; the placement stays
    /// centered on that monitor (multi-monitor + DPI safety of the session
    /// capsule's per-start positioning).
    #[test]
    fn bottom_center_in_is_origin_and_dpi_safe() {
        let area = (2000.0, 100.0, 2560.0, 1360.0);
        let (x, y) = bottom_center_in(area, WINDOW, 48.0 * 1.5);
        assert_eq!(x, 2000.0 + (2560.0 - WINDOW.0) / 2.0);
        assert_eq!(y, 100.0 + 1360.0 - WINDOW.1 - 72.0);
    }

    /// The session capsule is a window fully decoupled from the overlay: the
    /// two labels must never collide, or `get_webview_window` would hand
    /// back the wrong window and e.g. the tray overlay toggle could take the
    /// session capsule down.
    #[test]
    fn capsule_window_labels_are_distinct() {
        assert_ne!(
            crate::windowing::OVERLAY_WINDOW_LABEL,
            crate::windowing::SESSION_WINDOW_LABEL
        );
    }

    // ---- capsule-frame edge snapping -------------------------------------
    //
    // Fixed geometry for every snap test: window 280×64, work area
    // (100, 50, 1920, 1040) so the work-area origin is *not* (0, 0).

    const WINDOW: (f64, f64) = (280.0, 64.0);
    const AREA: (f64, f64, f64, f64) = (100.0, 50.0, 1920.0, 1040.0);
    const IDLE_CAPSULE: (f64, f64) = (104.0, 36.0); // standby capsule
    const SESSION_CAPSULE: (f64, f64) = (268.0, 52.0); // recording / processing capsule

    /// Capsule center line must land ON the work-area edge it docked to
    /// (≤ 2 physical px — half-hidden drawer landing, rounding slack only).
    /// The dock therefore always shows exactly half the capsule. The reported
    /// `edge` must also match the requested side: it is the renderer contract
    /// behind `set_overlay_position`'s `Some(edge)` return.
    fn assert_center_on_edge(
        window: (f64, f64),
        capsule: (f64, f64),
        snapped: SnapLanding,
        edge: &str,
    ) {
        assert_eq!(
            snapped.edge, edge,
            "{edge} dock: snap reported the wrong edge"
        );
        let offset_x = (window.0 - capsule.0) / 2.0;
        let offset_y = (window.1 - capsule.1) / 2.0;
        let center_x = snapped.pos.0 + offset_x + capsule.0 / 2.0;
        let center_y = snapped.pos.1 + offset_y + capsule.1 / 2.0;
        let distance = match edge {
            "left" => (center_x - AREA.0).abs(),
            "right" => (center_x - (AREA.0 + AREA.2)).abs(),
            "top" => (center_y - AREA.1).abs(),
            _ => (center_y - (AREA.1 + AREA.3)).abs(),
        };
        assert!(
            distance <= 2.0,
            "{edge} dock: capsule center line {distance} px from the work-area edge"
        );
    }

    /// Standby capsule (104×36) docks half-hidden on all four work-area edges:
    /// center line on the edge, capsule half outside the work area.
    #[test]
    fn idle_capsule_docks_half_hidden_on_every_edge() {
        // Drop positions are window coordinates whose *capsule* already hovers
        // near the target edge (inside the magnetic threshold).
        let left = snap_overlay_position(WINDOW, Some(IDLE_CAPSULE), AREA, 20.0, 500.0).unwrap();
        assert_center_on_edge(WINDOW, IDLE_CAPSULE, left, "left");
        // Half-hidden: the capsule straddles the work-area edge.
        let capsule_left = left.pos.0 + (WINDOW.0 - IDLE_CAPSULE.0) / 2.0;
        assert!(capsule_left < AREA.0 && capsule_left + IDLE_CAPSULE.0 > AREA.0);
        let right = snap_overlay_position(WINDOW, Some(IDLE_CAPSULE), AREA, 1840.0, 500.0).unwrap();
        assert_center_on_edge(WINDOW, IDLE_CAPSULE, right, "right");
        let top = snap_overlay_position(WINDOW, Some(IDLE_CAPSULE), AREA, 900.0, 60.0).unwrap();
        assert_center_on_edge(WINDOW, IDLE_CAPSULE, top, "top");
        let capsule_top = top.pos.1 + (WINDOW.1 - IDLE_CAPSULE.1) / 2.0;
        assert!(capsule_top < AREA.1 && capsule_top + IDLE_CAPSULE.1 > AREA.1);
        let bottom =
            snap_overlay_position(WINDOW, Some(IDLE_CAPSULE), AREA, 900.0, 1010.0).unwrap();
        assert_center_on_edge(WINDOW, IDLE_CAPSULE, bottom, "bottom");
    }

    /// Session capsule (268×52) docks half-hidden on all four work-area edges.
    #[test]
    fn session_capsule_docks_half_hidden_on_every_edge() {
        let left =
            snap_overlay_position(WINDOW, Some(SESSION_CAPSULE), AREA, 108.0, 500.0).unwrap();
        assert_center_on_edge(WINDOW, SESSION_CAPSULE, left, "left");
        let right =
            snap_overlay_position(WINDOW, Some(SESSION_CAPSULE), AREA, 1760.0, 500.0).unwrap();
        assert_center_on_edge(WINDOW, SESSION_CAPSULE, right, "right");
        let top = snap_overlay_position(WINDOW, Some(SESSION_CAPSULE), AREA, 900.0, 60.0).unwrap();
        assert_center_on_edge(WINDOW, SESSION_CAPSULE, top, "top");
        let bottom =
            snap_overlay_position(WINDOW, Some(SESSION_CAPSULE), AREA, 900.0, 1030.0).unwrap();
        assert_center_on_edge(WINDOW, SESSION_CAPSULE, bottom, "bottom");
    }

    /// Snapping only anchors the winning edge; the other axis is preserved.
    /// Both capsule states share one docked window position, so the capsule
    /// never jumps when it morphs between states.
    #[test]
    fn snap_only_moves_the_winning_axis() {
        let left = snap_overlay_position(WINDOW, Some(IDLE_CAPSULE), AREA, 20.0, 500.0).unwrap();
        assert!((left.pos.1 - 500.0).abs() <= 2.0);
        // Formula contract: windowX = workLeft - capsuleOffsetX - capsuleW/2
        // (= workLeft - windowW/2, independent of capsule size).
        let expected_x = AREA.0 - (WINDOW.0 - IDLE_CAPSULE.0) / 2.0 - IDLE_CAPSULE.0 / 2.0;
        assert!((left.pos.0 - expected_x).abs() <= 2.0);
        let left_session =
            snap_overlay_position(WINDOW, Some(SESSION_CAPSULE), AREA, 108.0, 500.0).unwrap();
        assert!(
            (left_session.pos.0 - left.pos.0).abs() <= 2.0,
            "both states share the dock"
        );
    }

    /// Beyond [`OVERLAY_SNAP_THRESHOLD`] from every edge: no snap at all —
    /// the caller keeps the drop (clamped). At the threshold it still snaps.
    #[test]
    fn drop_beyond_snap_threshold_does_not_snap() {
        // Capsule edge 50 px from the left edge (> 48): must not snap.
        assert_eq!(
            snap_overlay_position(WINDOW, Some(IDLE_CAPSULE), AREA, 62.0, 500.0),
            None
        );
        // Dead center: nearest edge is hundreds of px away.
        assert_eq!(
            snap_overlay_position(WINDOW, Some(IDLE_CAPSULE), AREA, 900.0, 500.0),
            None
        );
        // Exactly at the threshold (48 px) still snaps.
        let snapped = snap_overlay_position(WINDOW, Some(IDLE_CAPSULE), AREA, 60.0, 500.0).unwrap();
        assert_center_on_edge(WINDOW, IDLE_CAPSULE, snapped, "left");
    }

    /// No capsule parameter falls back to the historical window-frame
    /// behavior: dropped *point* picks the nearest edge, window edge lands
    /// [`OVERLAY_EDGE_GAP`] from it, unconditionally.
    #[test]
    fn without_capsule_falls_back_to_window_frame_behavior() {
        // Left: drop point 70 px from the work-area left → window at +10 px.
        assert_eq!(
            snap_overlay_position(WINDOW, None, AREA, 30.0, 500.0),
            Some(SnapLanding {
                pos: (AREA.0 + OVERLAY_EDGE_GAP, 500.0),
                edge: "left",
            })
        );
        // Right: drop point 20 px from the work-area right edge.
        assert_eq!(
            snap_overlay_position(WINDOW, None, AREA, 2000.0, 500.0),
            Some(SnapLanding {
                pos: (AREA.0 + AREA.2 - WINDOW.0 - OVERLAY_EDGE_GAP, 500.0),
                edge: "right",
            })
        );
        // Unconditional: far from every edge it still snaps to the nearest
        // one (no magnetic threshold in the legacy path).
        assert!(snap_overlay_position(WINDOW, None, AREA, 900.0, 500.0).is_some());
    }

    /// A degenerate or larger-than-window capsule is malformed input: it
    /// cannot define a visible frame, so the legacy snap takes over.
    #[test]
    fn malformed_capsule_sizes_fall_back_to_window_frame() {
        let legacy = snap_overlay_position(WINDOW, None, AREA, 30.0, 500.0);
        for bad in [
            (0.0, 36.0),
            (-104.0, 36.0),
            (104.0, 0.0),
            (300.0, 36.0),  // wider than the window
            (104.0, 128.0), // taller than the window
            (f64::NAN, 36.0),
            (104.0, f64::INFINITY),
        ] {
            assert_eq!(
                snap_overlay_position(WINDOW, Some(bad), AREA, 30.0, 500.0),
                legacy,
                "capsule {bad:?} must fall back to the window-frame snap"
            );
        }
    }

    /// Re-clamping a half-hidden dock (show / re-anchor path) must be a no-op:
    /// the capsule center line on the work-area edge is a legal position, so
    /// re-clamping may never pull it back.
    #[test]
    fn snapped_position_survives_reclamp() {
        let frame = OverlayFrame {
            area_x: AREA.0,
            area_y: AREA.1,
            area_w: AREA.2,
            area_h: AREA.3,
            window_w: WINDOW.0,
            window_h: WINDOW.1,
            monitor_scale: 1.0,
            window_scale: 1.0,
        };
        // Derive each drop from the desired dock instead of hand-picking
        // points shared by both capsules: the target-edge capsule side sits
        // DROP_INSET px *inside* the work-area edge, so the drop is always
        // within OVERLAY_SNAP_THRESHOLD (48) for every capsule size. A single
        // shared drop can snap for one capsule yet exceed the threshold for
        // the other (e.g. the left drop (20, 500) puts the session capsule's
        // left side 74 px from the work-area left — beyond 48 — so it legally
        // returns None while the standby capsule snaps).
        const DROP_INSET: f64 = 8.0;
        for edge in ["left", "right", "top", "bottom"] {
            for capsule in [IDLE_CAPSULE, SESSION_CAPSULE] {
                let offset_x = (WINDOW.0 - capsule.0) / 2.0;
                let offset_y = (WINDOW.1 - capsule.1) / 2.0;
                // Capsule side = target work-area edge ± DROP_INSET.
                let drop = match edge {
                    "left" => (AREA.0 + DROP_INSET - offset_x, 500.0),
                    "right" => (AREA.0 + AREA.2 - DROP_INSET - offset_x - capsule.0, 500.0),
                    "top" => (900.0, AREA.1 + DROP_INSET - offset_y),
                    _ => (900.0, AREA.1 + AREA.3 - DROP_INSET - offset_y - capsule.1),
                };
                let snapped =
                    snap_overlay_position(WINDOW, Some(capsule), AREA, drop.0, drop.1).unwrap();
                assert_eq!(
                    frame.clamp_center(snapped.pos.0, snapped.pos.1),
                    snapped.pos,
                    "{edge} dock ({capsule:?}) must survive the show-path re-clamp"
                );
            }
        }
    }

    /// Center-based clamp: any position whose *center* is inside the work area
    /// (including half-hidden hugging positions) is left alone; only a center
    /// outside the work area is pulled to the nearest edge — and never further,
    /// so the capsule is never fully hidden.
    #[test]
    fn clamp_center_keeps_near_edge_and_pulls_wayward_centers() {
        let frame = OverlayFrame {
            area_x: AREA.0,
            area_y: AREA.1,
            area_w: AREA.2,
            area_h: AREA.3,
            window_w: WINDOW.0,
            window_h: WINDOW.1,
            monitor_scale: 1.0,
            window_scale: 1.0,
        };
        // Half-hidden dock (center on the edge): unchanged.
        let docked_x = AREA.0 - WINDOW.0 / 2.0;
        assert_eq!(frame.clamp_center(docked_x, 500.0), (docked_x, 500.0));
        // Center slightly inside (window half outside the work area): unchanged.
        assert_eq!(
            frame.clamp_center(docked_x + 10.0, 500.0),
            (docked_x + 10.0, 500.0)
        );
        // Center outside the work area: pulled exactly onto the edge.
        assert_eq!(
            frame.clamp_center(docked_x - 30.0, 500.0),
            (docked_x, 500.0)
        );
        // Same on the vertical axis.
        let docked_y = AREA.1 + AREA.3 - WINDOW.1 / 2.0;
        assert_eq!(
            frame.clamp_center(900.0, docked_y + 30.0),
            (900.0, docked_y)
        );
        assert_eq!(
            frame.clamp_center(900.0, docked_y - 10.0),
            (900.0, docked_y - 10.0)
        );
    }

    /// The optional `capsule` argument is parsed defensively: malformed JSON
    /// shapes are ignored (→ legacy snap) and never error or panic.
    #[test]
    fn capsule_dims_ignores_malformed_arguments() {
        assert_eq!(
            capsule_dims(&Some(serde_json::json!({ "width": 104, "height": 36 }))),
            Some((104.0, 36.0))
        );
        assert_eq!(capsule_dims(&None), None);
        assert_eq!(capsule_dims(&Some(serde_json::json!(null))), None);
        assert_eq!(capsule_dims(&Some(serde_json::json!("wide"))), None);
        assert_eq!(
            capsule_dims(&Some(serde_json::json!({ "width": 104 }))),
            None
        );
        assert_eq!(
            capsule_dims(&Some(serde_json::json!({ "width": "104", "height": 36 }))),
            None
        );
    }

    /// `set_overlay_position`'s renderer-facing return contract
    /// (`Result<Option<String>, String>` → JSON `string | null`), resolved on
    /// the pure snap layer: a snapped drop yields the docked edge, a drop
    /// beyond the magnetic threshold yields `None` (renderer keeps the
    /// expanded capsule). The reset branch is the third variant — it never
    /// snaps, so it always returns `None` (see [`overlay_reset_patch`]).
    #[test]
    fn snap_reports_docked_edge_for_renderer_contract() {
        let left = snap_overlay_position(WINDOW, Some(IDLE_CAPSULE), AREA, 20.0, 500.0).unwrap();
        assert_eq!(left.edge, "left");
        let right = snap_overlay_position(WINDOW, Some(IDLE_CAPSULE), AREA, 1840.0, 500.0).unwrap();
        assert_eq!(right.edge, "right");
        let top = snap_overlay_position(WINDOW, Some(IDLE_CAPSULE), AREA, 900.0, 60.0).unwrap();
        assert_eq!(top.edge, "top");
        let bottom =
            snap_overlay_position(WINDOW, Some(IDLE_CAPSULE), AREA, 900.0, 1010.0).unwrap();
        assert_eq!(bottom.edge, "bottom");
        // Beyond the threshold → no snap → renderer receives `null` (expanded).
        assert_eq!(
            snap_overlay_position(WINDOW, Some(IDLE_CAPSULE), AREA, 900.0, 500.0),
            None
        );
        // The compact 36×36 capsule must re-dock from an already-docked spot:
        // a persist run while docked keeps reporting the edge (no flapping).
        const COMPACT: (f64, f64) = (36.0, 36.0);
        let docked_x = AREA.0 - (WINDOW.0 - COMPACT.0) / 2.0 - COMPACT.0 / 2.0;
        let again = snap_overlay_position(WINDOW, Some(COMPACT), AREA, docked_x, 500.0).unwrap();
        assert_eq!(again.edge, "left");
    }

    /// Dock state persists next to the position: a snapped drop stores the
    /// edge, a clamped-only drop stores `null`, and the reset path clears
    /// both. `position_overlay_persisted` (position) + `getConfig`
    /// (`overlayDocked`) replay exactly these two fields across a restart.
    #[test]
    fn config_patches_persist_dock_state_with_position() {
        let docked = overlay_position_patch((100.0, 500.0), Some("left"));
        assert_eq!(
            docked["app"]["overlayPosition"]["x"].as_f64(),
            Some(100.0),
            "position must be stored alongside the dock"
        );
        assert_eq!(docked["app"]["overlayDocked"].as_str(), Some("left"));

        let clamped_only = overlay_position_patch((900.0, 500.0), None);
        assert!(clamped_only["app"]["overlayPosition"]["x"].is_number());
        assert!(
            clamped_only["app"]["overlayDocked"].is_null(),
            "a clamped-only drop must clear the dock"
        );

        let reset = overlay_reset_patch();
        assert!(reset["app"]["overlayPosition"].is_null());
        assert!(reset["app"]["overlayDocked"].is_null());
    }

    /// 0.5.7 regression (user report "拖过头反而不吸"): a dropped capsule whose
    /// side has **crossed** a work-area edge must dock on that edge instead of
    /// being rejected. The old `abs()` distance folded an overflow back into
    /// the `0..=48` window from the other side, so any overflow deeper than 48
    /// px (e.g. the capsule half past the edge = 52 px) read as "far away" and
    /// returned `None`. Signed distances fix it: every negative distance
    /// qualifies, however deep — overflow is a stronger edge signal than a
    /// touch. Covered here for all four edges at two depths (10 px past = the
    /// user's "拖过头 20px" scenario band; 100 px past = far beyond the 48
    /// threshold) and both capsule states, and each landing must survive the
    /// show-path re-clamp (the decision runs on the raw dropped coordinates).
    #[test]
    fn overflowing_drop_still_docks_on_every_edge() {
        let frame = OverlayFrame {
            area_x: AREA.0,
            area_y: AREA.1,
            area_w: AREA.2,
            area_h: AREA.3,
            window_w: WINDOW.0,
            window_h: WINDOW.1,
            monitor_scale: 1.0,
            window_scale: 1.0,
        };
        for overhang in [10.0, 100.0] {
            for edge in ["left", "right", "top", "bottom"] {
                for cap in [IDLE_CAPSULE, SESSION_CAPSULE] {
                    let ox = (WINDOW.0 - cap.0) / 2.0;
                    let oy = (WINDOW.1 - cap.1) / 2.0;
                    // The capsule's target side sits `overhang` px *outside*
                    // the work-area edge (window coords, capsule centered).
                    let drop = match edge {
                        "left" => (AREA.0 - overhang - ox, 500.0),
                        "right" => (AREA.0 + AREA.2 + overhang - ox - cap.0, 500.0),
                        "top" => (900.0, AREA.1 - overhang - oy),
                        _ => (900.0, AREA.1 + AREA.3 + overhang - oy - cap.1),
                    };
                    let snap = snap_overlay_position(WINDOW, Some(cap), AREA, drop.0, drop.1);
                    assert!(snap.is_some(), "{edge} overhang {overhang} must dock");
                    let snapped = snap.unwrap();
                    assert_center_on_edge(WINDOW, cap, snapped, edge);
                    assert_eq!(
                        frame.clamp_center(snapped.pos.0, snapped.pos.1),
                        snapped.pos,
                        "{edge} overflow landing must survive the show-path re-clamp"
                    );
                }
            }
        }
    }

    /// Multi-edge drops pick the strongest edge signal: the smallest signed
    /// distance — a deep overflow beats a merely close touch on another axis —
    /// and an exact tie keeps the historical left > right > top > bottom
    /// priority (the if-chain order in `snap_overlay_position`). With the old
    /// `abs()` distances the first case wrongly docked **top** (|−100| > 20
    /// lost the min) instead of the overflowed right edge.
    #[test]
    fn deepest_edge_signal_wins_and_ties_keep_historical_priority() {
        // Overhanging the right edge by 100 px while only 20 px inside the top
        // edge: the overflow is the stronger signal → docks right.
        let hit = snap_overlay_position(WINDOW, Some(IDLE_CAPSULE), AREA, 1928.0, 56.0);
        let snapped = hit.unwrap();
        assert_center_on_edge(WINDOW, IDLE_CAPSULE, snapped, "right");
        // Exact tie (30 px inside the left and top edges): left wins.
        let hit = snap_overlay_position(WINDOW, Some(IDLE_CAPSULE), AREA, 42.0, 66.0);
        let tied = hit.unwrap();
        assert_center_on_edge(WINDOW, IDLE_CAPSULE, tied, "left");
    }

    /// The compact docked capsule must re-dock from its own docked spot on
    /// every edge (0.5.6 A-补 contract, `snap_reports_docked_edge_for_renderer_contract`
    /// extended to all four edges): a persist run while docked keeps reporting
    /// the edge instead of flapping to the expanded capsule. The docked spot
    /// is itself an overflow state (center line on the edge → capsule half
    /// out), so this also locks the signed-distance fix against a revert.
    #[test]
    fn compact_docked_capsule_redocks_on_every_edge() {
        const COMPACT: (f64, f64) = (36.0, 36.0);
        let offset_x = (WINDOW.0 - COMPACT.0) / 2.0;
        let offset_y = (WINDOW.1 - COMPACT.1) / 2.0;
        for edge in ["left", "right", "top", "bottom"] {
            // Feed the exact half-hidden landing for this edge back as a drop.
            let docked = match edge {
                "left" => (AREA.0 - offset_x - COMPACT.0 / 2.0, 500.0),
                "right" => (AREA.0 + AREA.2 - offset_x - COMPACT.0 / 2.0, 500.0),
                "top" => (900.0, AREA.1 - offset_y - COMPACT.1 / 2.0),
                _ => (900.0, AREA.1 + AREA.3 - offset_y - COMPACT.1 / 2.0),
            };
            let snap = snap_overlay_position(WINDOW, Some(COMPACT), AREA, docked.0, docked.1);
            assert!(snap.is_some(), "docked {edge} capsule must re-dock");
            let again = snap.unwrap();
            assert_center_on_edge(WINDOW, COMPACT, again, edge);
        }
    }
}
