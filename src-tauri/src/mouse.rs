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
//! * **Wheel is inert.** The callback acts on `WM_MBUTTONDOWN` /
//!   `WM_MBUTTONUP` and nothing else, so `WM_MOUSEWHEEL` / `WM_MOUSEHWHEEL` (and
//!   every other mouse message) can never start, stop or otherwise disturb a
//!   held session — scrolling mid-press leaves the down/up pair intact. The
//!   message→action decision is the pure `win::classify` whitelist, locked by
//!   regression tests; see its comment for the `WM_MOUSELAST`/wheel range trap
//!   it guards.
//! * **Bounce-tolerant release.** Scrolling the wheel vibrates the middle
//!   switch, and a mechanical switch can bounce *open* for a moment: Windows
//!   then delivers a real `WM_MBUTTONUP` (immediately followed by another
//!   `WM_MBUTTONDOWN`) although the user still holds the button. Before a
//!   release stops anything the worker re-checks the physical key state
//!   (`GetAsyncKeyState(VK_MBUTTON)`, high bit 0x8000 = "currently held"):
//!   still held → the release is ignored as a bounce and both the session and
//!   the ownership token are kept, so the genuine release still stops later;
//!   actually released → the stop proceeds exactly as before. The re-check
//!   must stay on the worker, never in the hook callback: the queue delay is
//!   exactly what lets the switch already be re-pressed when the spurious Up
//!   is handled, and the hook keeps its latency budget. The `Disable` stop
//!   decision is never re-checked (it must always stop); worst case (a
//!   genuine release misread as a bounce leaves a session running) the
//!   5-minute session cap — doubly backed by the renderer's `setTimeout` and
//!   the Rust watchdog (`commands::spawn_session_time_watchdog`) — and the
//!   tray Disable path still end it. Decision logic is the pure
//!   `win::should_treat_as_bounce`, locked by tests.
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
//!   same stop decision as a button release (minus the bounce re-check) so the
//!   recording cannot orphan.
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
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_MBUTTON};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetAncestor, PeekMessageW, SetWindowsHookExW,
        TranslateMessage, UnhookWindowsHookEx, WindowFromPoint, GA_ROOT, HHOOK, MSG,
        MSLLHOOKSTRUCT, PM_REMOVE, WH_MOUSE_LL, WM_MBUTTONDOWN, WM_MBUTTONUP,
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
        /// The setting was turned off (possibly mid-press): same stop decision
        /// as a release (but never bounce-re-checked), so an owned recording
        /// cannot be orphaned.
        Disable,
    }

    /// Whether the stop decision came from a real button release or from the
    /// setting turning off: only a real release is bounce-re-checked (see
    /// [`handle_middle_up`]).
    #[derive(Debug, Copy, Clone, PartialEq, Eq)]
    enum StopCause {
        /// `WM_MBUTTONUP`: re-check the physical key state first.
        Release,
        /// Setting turned off mid-press: stop unconditionally.
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
                MiddleEvent::Up => handle_middle_up(StopCause::Release),
                MiddleEvent::Disable => handle_middle_up(StopCause::Disable),
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
                match id {
                    Some(id) => {
                        // Boundary instrumentation: a hardware switch bounce
                        // (spurious release while scrolling) shows up as this
                        // line, then "bounce-tolerant release ignored" and a
                        // "start rejected" line for the re-press; a genuine
                        // release shows up as the release/stop line instead.
                        log::debug!("mouse: middle-button press started session {id}");
                        set_owned_session(Some(id));
                    }
                    None => {
                        clear_owned_session();
                        log::warn!("mouse: started a session but could not read its id; release will be a no-op");
                    }
                }
            }
            Err(e) => log::debug!("mouse: start rejected: {e}"),
        }
    }

    /// Worker-side `WM_MBUTTONUP` / disable: stop only the session this hook
    /// started and that is still recording; anything else is a no-op.
    ///
    /// A real release first re-checks the physical key state (bounce
    /// tolerance); [`StopCause::Disable`] keeps its original semantics and
    /// stops unconditionally. On a bounce this returns *before* taking the
    /// ownership token, so the genuine release can still stop the session.
    fn handle_middle_up(cause: StopCause) {
        if cause == StopCause::Release {
            // Hardware-switch bounce (e.g. the vibration of scrolling the
            // wheel while the button is held) delivers a real WM_MBUTTONUP
            // while the finger still holds the switch. The queue delay is
            // exactly what makes this re-check reliable: by the time the
            // worker handles the spurious Up, the following press has usually
            // re-asserted the key physically. Never move this re-check into
            // the hook callback: there the spurious Up would still see the
            // momentarily open switch (and the hook must keep its latency
            // budget). Worst case (a genuine release misread as a bounce
            // leaves a session running) the 5-minute session cap — renderer
            // `setTimeout` and the Rust watchdog in `commands.rs`, dual
            // backstops — and the tray Disable path still end the recording.
            if should_treat_as_bounce(middle_button_key_state()) {
                log::debug!("mouse: bounce-tolerant release ignored");
                return;
            }
        }
        // Taking the token clears ownership unconditionally, as required for
        // every Stop decision.
        let owned_id = OWNED_SESSION
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
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
            log::debug!(
                "mouse: middle-button release ignored (owned session {owned_id} is no longer recording)"
            );
            return;
        }
        log::debug!("mouse: middle-button release stopping session {owned_id}");
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

    /// `GetAsyncKeyState` high bit = "the key is currently down". The low bit
    /// (0x01, "pressed since the last query") is history, not physical state.
    const KEY_STATE_DOWN_HIGH_BIT: u16 = 0x8000;

    /// Raw `GetAsyncKeyState(VK_MBUTTON)` result; see [`should_treat_as_bounce`]
    /// for how the bits are read. One cheap, non-blocking Win32 call — safe
    /// here on the worker, but never to be called from the hook callback.
    fn middle_button_key_state() -> i16 {
        // SAFETY: reading shared input state cannot fail and does not block.
        unsafe { GetAsyncKeyState(VK_MBUTTON as i32) }
    }

    /// Pure bounce decision for a middle-button release: the release is a
    /// hardware-switch bounce and must be ignored while the physical key is
    /// still held — decided by the high bit (0x8000) of the raw
    /// `GetAsyncKeyState` state, never by the low "pressed since the last
    /// query" bit. Pure so the release semantics are unit-testable without a
    /// real hook or keyboard.
    fn should_treat_as_bounce(key_state: i16) -> bool {
        (key_state as u16 & KEY_STATE_DOWN_HIGH_BIT) != 0
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
        win.hwnd()
            .map(|hwnd| hwnd.0 as isize == root_hwnd)
            .unwrap_or(false)
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

    /// The session action a hook message may cause. `None` (from [`classify`])
    /// means "no action": the message is ignored entirely.
    #[derive(Debug, Copy, Clone, PartialEq, Eq)]
    enum Trigger {
        /// `WM_MBUTTONDOWN`: begin a middle-button push-to-talk session.
        Down,
        /// `WM_MBUTTONUP`: end it (bounce-tolerant: the worker re-checks the
        /// physical key state first, see [`should_treat_as_bounce`]).
        Up,
    }

    /// Strict message-type whitelist: `WM_MBUTTONDOWN` / `WM_MBUTTONUP` and
    /// nothing else. Pure so it can be unit-tested without a real hook.
    ///
    /// **Regression trap.** `WM_MOUSEWHEEL` (522) and `WM_MOUSEHWHEEL` (526)
    /// sit *above* `WM_MBUTTONDOWN`/`WM_MBUTTONUP` (519/520) and *inside*
    /// `WM_MOUSEFIRST..=WM_MOUSELAST` (512..=526). Widening this test in any way
    /// — a range check (`WM_MBUTTONDOWN..=WM_MOUSELAST`), a threshold
    /// (`message >= WM_MBUTTONDOWN`), the whole `WM_MOUSEFIRST..=WM_MOUSELAST`
    /// span, or a "everything except `WM_MOUSEMOVE`" exclusion — silently maps
    /// the wheel to Down/Up and re-introduces the reported bug where rolling the
    /// wheel during a held middle button interrupts the recording. The low-level
    /// hook receives the *message id* in `wParam` (never button state: for a
    /// wheel, the high word of a window's `wParam` is the delta, but that rule
    /// does not apply here). Only an exact `==` match keeps scrolling harmless
    /// and the down/up pair intact.
    fn classify(message: u32) -> Option<Trigger> {
        // `as u32` on both sides keeps this independent of how windows-sys
        // types the WM_* / hook-id constants.
        if message == WM_MBUTTONDOWN as u32 {
            Some(Trigger::Down)
        } else if message == WM_MBUTTONUP as u32 {
            Some(Trigger::Up)
        } else {
            None
        }
    }

    /// `WH_MOUSE_LL` callback. Observing only: `CallNextHookEx` always runs so
    /// the click keeps its normal meaning in the target application.
    ///
    /// Latency-critical (the system removes slow hooks): only local Win32
    /// calls plus a non-blocking enqueue — no Tauri calls, no locks, no I/O.
    unsafe extern "system" fn low_level_mouse_proc(
        code: i32,
        wparam: usize,
        lparam: isize,
    ) -> isize {
        if code >= 0 && lparam != 0 {
            match classify(wparam as u32) {
                Some(Trigger::Down) => {
                    let info = &*(lparam as *const MSLLHOOKSTRUCT);
                    post_event(MiddleEvent::Down {
                        root_hwnd: root_window_at(info.pt),
                    });
                }
                Some(Trigger::Up) => post_event(MiddleEvent::Up),
                // Wheel / move / other buttons: intentionally inert, so scrolling
                // cannot disturb an in-flight middle-button PTT session.
                None => {}
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

    #[cfg(test)]
    mod tests {
        use super::*;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDBLCLK, WM_MOUSEFIRST, WM_MOUSEHWHEEL,
            WM_MOUSELAST, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP,
            WM_XBUTTONDOWN, WM_XBUTTONUP,
        };

        /// The only messages that may drive a middle-button PTT session.
        #[test]
        fn middle_button_messages_map_to_down_and_up() {
            assert_eq!(classify(WM_MBUTTONDOWN as u32), Some(Trigger::Down));
            assert_eq!(classify(WM_MBUTTONUP as u32), Some(Trigger::Up));
        }

        /// Regression: rolling the wheel — vertical (`WM_MOUSEWHEEL`) or
        /// horizontal (`WM_MOUSEHWHEEL`) — during a held middle button must
        /// produce no action at all, so the held down/up pair stays intact and
        /// the recording is never interrupted.
        #[test]
        fn wheel_messages_are_inert() {
            assert_eq!(classify(WM_MOUSEWHEEL as u32), None);
            assert_eq!(classify(WM_MOUSEHWHEEL as u32), None);
        }

        /// Regression: every other mouse message (move, middle double-click,
        /// left/right, X buttons) is inert too — the hook only ever tracks the
        /// middle button press/release.
        #[test]
        fn other_mouse_messages_are_inert() {
            for message in [
                WM_MOUSEMOVE,
                WM_MBUTTONDBLCLK,
                WM_LBUTTONDOWN,
                WM_LBUTTONUP,
                WM_RBUTTONDOWN,
                WM_RBUTTONUP,
                WM_XBUTTONDOWN,
                WM_XBUTTONUP,
            ] {
                assert_eq!(
                    classify(message as u32),
                    None,
                    "message {message:#06x} must be inert"
                );
            }
        }

        /// Documents *why* [`classify`] must stay an exact `==` whitelist and
        /// never become a range/threshold test: the wheel messages live between
        /// `WM_MBUTTONDOWN` and `WM_MOUSELAST`, so a widened check would catch
        /// them and re-introduce the "scrolling interrupts recording" bug that
        /// these tests exist to prevent.
        #[test]
        fn wheel_messages_sit_inside_the_range_trap() {
            for wheel in [WM_MOUSEWHEEL as u32, WM_MOUSEHWHEEL as u32] {
                assert!(wheel > WM_MBUTTONUP as u32 && wheel <= WM_MOUSELAST as u32);
                assert!((WM_MOUSEFIRST..=WM_MOUSELAST).contains(&wheel));
            }
        }

        /// Regression (hardware bounce): while the physical middle button is
        /// still held — high bit (0x8000) of `GetAsyncKeyState` set — a
        /// release must be treated as a bounce and must NOT stop the session:
        /// the worker keeps the session and the ownership token so the genuine
        /// release can still stop later.
        #[test]
        fn release_while_still_held_is_a_bounce_and_must_not_stop() {
            assert!(should_treat_as_bounce(0x8000u16 as i16));
            // High bit wins even alongside other bits (low-bit history).
            assert!(should_treat_as_bounce(0x8001u16 as i16));
        }

        /// A genuinely released key (high bit clear) must trigger the stop.
        /// The low bit (0x01, "pressed since the last query") is history, not
        /// physical state, and must never read as "still held".
        #[test]
        fn release_after_genuine_release_triggers_stop() {
            assert!(!should_treat_as_bounce(0));
            assert!(!should_treat_as_bounce(0x0001u16 as i16));
        }
    }
}
