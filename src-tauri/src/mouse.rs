//! Middle mouse button push-to-talk (Windows only).
//!
//! A dedicated background thread owns a low-level mouse hook
//! (`SetWindowsHookExW(WH_MOUSE_LL)`) and its message pump. `WM_MBUTTONDOWN`
//! starts a recording session and `WM_MBUTTONUP` stops it — the same
//! press/release semantics as the keyboard PTT binding in [`crate::hotkey`].
//!
//! Design notes:
//!
//! * **Observing only.** The hook never swallows input: every event is passed
//!   on through `CallNextHookEx`, so middle-click paste in terminals and other
//!   middle-button behaviors keep working (they may fire *together* with the
//!   recording trigger — the settings copy warns about this).
//! * **Hook latency budget.** `WH_MOUSE_LL` callbacks must return quickly (the
//!   system silently removes hooks that block, ~300 ms), so the hook procedure
//!   only performs *local* Win32 calls (the `WindowFromPoint`/`GetAncestor`
//!   hit test) and a non-blocking channel send. Every Tauri call (focus guard,
//!   overlay lookup, session start) runs on a **single serial worker** task.
//! * **Serial worker + session token.** Mouse events are queued (mpsc) and
//!   processed strictly in order by one async task. A Start that succeeds
//!   records the id of the session it started (the ownership token); a Stop
//!   only stops a session when `session.id == token && status == Recording`
//!   and then clears the token. Every other start / stop / cancel / error path
//!   clears the token too (`clear_owned_session`), so a stale release can never
//!   kill a session it does not own (e.g. keyboard-PTT session S2 after a
//!   middle-click session S1 ended early) and never produces a bogus
//!   "no recording in progress" error toast. Because Start/Stop decisions are
//!   serialized, a fast click (down immediately followed by up) can never leave
//!   an orphan recording: the Stop is decided after its Start, always.
//!   The long-running finalize (transcribe) is spawned detached so the worker
//!   keeps draining events; a Start during `Processing` is then rejected by
//!   `commands::start_session_internal` (W3) instead of queueing behind the
//!   transcription and firing minutes later.
//! * **Same guards as keyboard PTT.** No trigger while the main window is
//!   focused (`hotkey::main_window_focused`) or while the settings hotkey
//!   recorder owns the keyboard (`AppState::hotkey_capture_active`). The
//!   guards run on the worker, before the start.
//! * **De-dup with the HUD capsule.** Middle clicks that land on the always-on
//!   overlay capsule belong to its pointer handlers (feature A wins); the hook
//!   records the root window under the cursor with pure Win32 calls and the
//!   worker compares it against the overlay's `hwnd`.
//! * **Thread-safe toggling.** The hook handle is only ever touched by the pump
//!   thread (single owner): `set_middle_click_ptt` flips an atomic and the
//!   thread installs/removes the hook itself, so enable/disable can never race
//!   `SetWindowsHookExW`/`UnhookWindowsHookEx`. Disabling mid-press queues the
//!   same stop decision as a button release so the recording cannot orphan.
//!
//! Non-Windows builds compile to no-op stubs; no extra crate is required
//! (`windows-sys` with `Win32_UI_WindowsAndMessaging` already ships).

#[cfg(not(windows))]
pub fn init(_app: &tauri::AppHandle) {}

#[cfg(not(windows))]
pub fn set_middle_click_ptt(_enabled: bool) {}

#[cfg(not(windows))]
pub fn clear_owned_session() {}

#[cfg(windows)]
pub use win::{clear_owned_session, init, set_middle_click_ptt};

#[cfg(windows)]
mod win {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, OnceLock, PoisonError};
    use std::time::Duration;

    use tauri::{AppHandle, Manager};
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetAncestor, PeekMessageW, SetWindowsHookExW,
        TranslateMessage, UnhookWindowsHookEx, WindowFromPoint, GA_ROOT, HHOOK, MSLLHOOKSTRUCT,
        MSG, PM_REMOVE, WH_MOUSE_LL, WM_MBUTTONDOWN, WM_MBUTTONUP,
    };

    /// Process-wide app handle shared with the hook procedure (a plain
    /// `extern "system"` callback cannot capture state).
    static APP: OnceLock<AppHandle> = OnceLock::new();

    /// Desired state of the middle-click push-to-talk hook; mirrors the
    /// persisted `mouse.middleClickPtt` config.
    static ENABLED: AtomicBool = AtomicBool::new(false);

    /// Spawn guard: the pump thread runs for the whole process lifetime and
    /// only installs/removes the hook as [`ENABLED`] flips.
    static THREAD_RUNNING: AtomicBool = AtomicBool::new(false);

    /// Hook -> serial worker event queue. Unbounded and non-blocking on the
    /// send side so the hook procedure never waits; FIFO order is what makes
    /// "down then up" (fast click) deterministic.
    static EVENT_TX: OnceLock<UnboundedSender<MiddleEvent>> = OnceLock::new();

    /// Ownership token: id of the session the middle button press started.
    /// Only the worker's Stop decision reads it; any start/stop/cancel/error
    /// path may clear it via [`clear_owned_session`]. `None` means "the release
    /// must not stop anything".
    static OWNED_SESSION: Mutex<Option<String>> = Mutex::new(None);

    /// Middle-button events forwarded from the hook to the serial worker.
    enum MiddleEvent {
        /// `WM_MBUTTONDOWN`. `root_hwnd` is the top-level window under the
        /// cursor at event time (`GetAncestor(WindowFromPoint(..), GA_ROOT)`),
        /// computed with pure Win32 calls in the hook; the worker compares it
        /// with the overlay window handle.
        Down { root_hwnd: isize },
        /// `WM_MBUTTONUP`.
        Up,
        /// The setting was turned off (possibly mid-press): same decision as a
        /// release, so an owned recording cannot be orphaned.
        Disable,
    }

    /// Poll interval while the hook is armed (keeps hook latency low) and
    /// while disarmed (cheap idle).
    const POLL_ARMED: Duration = Duration::from_millis(5);
    const POLL_DISARMED: Duration = Duration::from_millis(50);

    /// Remember the app handle used by the hook procedure and start the serial
    /// worker that performs all Tauri-side work. Idempotent.
    pub fn init(app: &AppHandle) {
        let _ = APP.set(app.clone());
        spawn_worker();
    }

    /// Apply the `mouse.middleClickPtt` setting. Idempotent and thread-safe;
    /// effective within one poll tick (≤ 5 ms while turning on).
    pub fn set_middle_click_ptt(enabled: bool) {
        let was_enabled = ENABLED.swap(enabled, Ordering::SeqCst);
        if was_enabled && !enabled {
            // Disabling while the button still owns a recording must not leave
            // it running forever (no release event will be delivered once the
            // hook is gone): queue the same stop decision a release makes.
            post_event(MiddleEvent::Disable);
        }
        if enabled && !THREAD_RUNNING.swap(true, Ordering::SeqCst) {
            if let Err(e) = std::thread::Builder::new()
                .name("voicekey-mouse-ptt".into())
                .spawn(pump_loop)
            {
                THREAD_RUNNING.store(false, Ordering::SeqCst);
                log::error!("mouse: failed to spawn hook thread: {e}");
            }
        }
    }

    /// Drop the ownership token. Called from every session start / stop /
    /// cancel / error path outside this hook (`commands.rs`) so a later middle
    /// release can only ever stop a session this hook started itself.
    pub fn clear_owned_session() {
        *OWNED_SESSION.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    fn set_owned_session(id: Option<String>) {
        *OWNED_SESSION.lock().unwrap_or_else(PoisonError::into_inner) = id;
    }

    /// Non-blocking enqueue; safe to call from the hook procedure.
    fn post_event(event: MiddleEvent) {
        if let Some(tx) = EVENT_TX.get() {
            let _ = tx.send(event);
        }
    }

    /// Start the single serial worker. Runs for the whole process lifetime.
    fn spawn_worker() {
        if EVENT_TX.get().is_some() {
            return;
        }
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        if EVENT_TX.set(tx).is_err() {
            return;
        }
        tauri::async_runtime::spawn(worker_loop(rx));
    }

    async fn worker_loop(mut rx: UnboundedReceiver<MiddleEvent>) {
        while let Some(event) = rx.recv().await {
            match event {
                MiddleEvent::Down { root_hwnd } => handle_middle_down(root_hwnd),
                MiddleEvent::Up | MiddleEvent::Disable => handle_middle_up(),
            }
        }
    }

    /// Worker-side `WM_MBUTTONDOWN`: de-dup with the HUD capsule, apply the
    /// keyboard-PTT guards, start the session and remember its id.
    fn handle_middle_down(root_hwnd: isize) {
        let Some(app) = APP.get() else {
            return;
        };
        // Middle clicks landing on the HUD capsule belong to its pointer
        // handlers (the always-on overlay owns that geometry).
        if point_over_overlay(app, root_hwnd) {
            return;
        }
        let state = app.state::<crate::state::AppState>();
        // Same guards as the keyboard PTT binding (hotkey.rs): app-external
        // only, never while the settings hotkey recorder owns the keyboard.
        if state.hotkey_capture_active() || crate::hotkey::main_window_focused(app) {
            return;
        }
        // `start_session_internal` rejects Recording / Processing sessions
        // itself (W3); a rejection is a silent no-op here, exactly like the
        // keyboard PTT path.
        match crate::commands::start_session_internal(app) {
            Ok(payload) => {
                // Ownership token = the session WE just started (id from the
                // start payload; fall back to the session state). The release
                // may only stop this exact session.
                let id = payload
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
                    .or_else(|| {
                        state
                            .session
                            .lock()
                            .map(|s| s.id.clone())
                            .ok()
                            .filter(|id| !id.is_empty())
                    });
                if id.is_some() {
                    set_owned_session(id);
                } else {
                    clear_owned_session();
                    log::warn!("mouse: started a session but could not read its id; release will be a no-op");
                }
            }
            Err(e) => log::debug!("mouse: start rejected: {e}"),
        }
    }

    /// Worker-side `WM_MBUTTONUP` / disable: stop only the session this hook
    /// started and that is still recording; anything else is a no-op.
    fn handle_middle_up() {
        // Taking the token clears ownership unconditionally, as required for
        // every Stop decision.
        let owned_id = OWNED_SESSION.lock().unwrap_or_else(PoisonError::into_inner).take();
        let Some(owned_id) = owned_id else {
            return;
        };
        let Some(app) = APP.get() else {
            return;
        };
        let still_ours = app
            .state::<crate::state::AppState>()
            .session
            .lock()
            .map(|s| s.id == owned_id && s.status == crate::audio::SessionStatus::Recording)
            .unwrap_or(false);
        if !still_ours {
            // The session already ended on its own or was replaced (e.g.
            // keyboard PTT started a new one): never stop a foreign session
            // and never emit a bogus "no recording" error toast.
            return;
        }
        // The hook must return fast and the worker must keep draining events
        // in order (so a queued Start is decided against the current state,
        // not against a transcription that may run for a minute): run the
        // finalize detached.
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = crate::commands::stop_session_internal(&handle).await {
                log::warn!("mouse: stop_session_internal failed: {e}");
            }
        });
    }

    /// True when the given root window (top-level window under the cursor at
    /// event time) is the HUD overlay. The hit test itself ran in the hook
    /// with pure Win32 calls; only this comparison needs Tauri (overlay
    /// `hwnd`), so it happens here on the worker. A hidden capsule can never
    /// match: a hidden window is not the root window under the cursor.
    fn point_over_overlay(app: &AppHandle, root_hwnd: isize) -> bool {
        let Some(win) = app.get_webview_window(crate::windowing::OVERLAY_WINDOW_LABEL) else {
            return false;
        };
        // `hwnd.0` is the raw pointer windows-sys expects (see windowing.rs).
        win.hwnd().map(|hwnd| hwnd.0 as isize == root_hwnd).unwrap_or(false)
    }

    /// Hook thread body: keep the message queue pumped (that is what delivers
    /// low-level hook callbacks) and install/remove the hook as the setting
    /// flips. The hook handle never leaves this thread.
    fn pump_loop() {
        let mut hook: HHOOK = std::ptr::null_mut();
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        loop {
            let want = ENABLED.load(Ordering::SeqCst);
            if want && hook.is_null() {
                hook = unsafe {
                    SetWindowsHookExW(
                        WH_MOUSE_LL as _,
                        Some(low_level_mouse_proc),
                        std::ptr::null_mut(),
                        0,
                    )
                };
                if hook.is_null() {
                    // Do not retry in a hot loop; the setting can be toggled
                    // off/on from settings to try again.
                    log::error!(
                        "mouse: SetWindowsHookExW(WH_MOUSE_LL) failed; middle-click push-to-talk is off"
                    );
                    ENABLED.store(false, Ordering::SeqCst);
                }
            } else if !want && !hook.is_null() {
                unsafe { UnhookWindowsHookEx(hook) };
                hook = std::ptr::null_mut();
            }

            // Drain the thread message queue. Retrieving messages is also what
            // makes the system call the low-level hook on this thread.
            while unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
                unsafe {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }

            std::thread::sleep(if ENABLED.load(Ordering::SeqCst) {
                POLL_ARMED
            } else {
                POLL_DISARMED
            });
        }
    }

    /// `WH_MOUSE_LL` callback. Observing only: `CallNextHookEx` always runs so
    /// the click keeps its normal meaning in the target application.
    ///
    /// Latency-critical (the system removes slow hooks): only local Win32
    /// calls plus a non-blocking enqueue — no Tauri calls, no locks, no I/O.
    unsafe extern "system" fn low_level_mouse_proc(code: i32, wparam: usize, lparam: isize) -> isize {
        if code >= 0 && lparam != 0 {
            // `as u32` on both sides keeps this independent of how windows-sys
            // types the WM_* / hook-id constants.
            let message = wparam as u32;
            if message == WM_MBUTTONDOWN as u32 {
                let info = &*(lparam as *const MSLLHOOKSTRUCT);
                post_event(MiddleEvent::Down {
                    root_hwnd: root_window_at(info.pt),
                });
            } else if message == WM_MBUTTONUP as u32 {
                post_event(MiddleEvent::Up);
            }
        }
        CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
    }

    /// Pure Win32 hit test: the top-level window under the desktop point,
    /// captured at event time (so it cannot change before the worker looks at
    /// it). Child windows (WebView2) resolve to their top-level owner.
    fn root_window_at(point: POINT) -> isize {
        unsafe { GetAncestor(WindowFromPoint(point), GA_ROOT as _) as isize }
    }
}
