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
//! * **Bounce-tolerant release — event-driven, never key-state-driven.**
//!   Scrolling the wheel vibrates the middle switch, and a mechanical switch
//!   can chatter *open* for a moment: Windows then delivers a real
//!   `WM_MBUTTONUP` immediately followed by another `WM_MBUTTONDOWN`, although
//!   the finger still holds the button. The worker therefore never acts on a
//!   release at once: it waits out a short grace window
//!   (`win::RELEASE_BOUNCE_WINDOW`) for that re-press. A press inside the
//!   window is the causal proof that the switch never really opened — the
//!   release is absorbed as chatter and both the session and the ownership
//!   token are kept, so the genuine release still stops later. No press inside
//!   the window (or a press only after it) is a genuine release and the stop
//!   proceeds. Chatter can thus only *delay* a stop, never veto it.
//!
//!   **Regression trap (P0: "middle-button release never stops").** The first
//!   attempt gated the release on `GetAsyncKeyState(VK_MBUTTON)` (high bit =
//!   "currently held") read on the worker, "vetoing" the stop while held. That
//!   signal is unusable for the event being handled: a low-level hook callback
//!   runs *before* the asynchronous key state is updated (documented on
//!   `LowLevelKeyboardProc`/`LowLevelMouseProc`), so reading it from the worker
//!   right after the enqueue still observes the pre-event (held) state on real
//!   hardware — every genuine release was swallowed as a bounce and the
//!   recording only ended at the 5-minute cap. The event *pair* is the causal
//!   evidence; a sampled key state is not. Decision logic is the pure
//!   `win::release_after_bounce_window`, locked by tests. The `Disable` stop
//!   decision never enters the grace window (it must always stop).
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
        /// as a release (but never held back by the bounce grace window), so an
        /// owned recording cannot be orphaned.
        Disable,
    }

    /// Whether the stop decision came from a real button release or from the
    /// setting turning off: only a real release goes through the bounce grace
    /// window (see [`handle_deferred_release`]).
    #[derive(Debug, Copy, Clone, PartialEq, Eq)]
    enum StopCause {
        /// `WM_MBUTTONUP`, already past the bounce grace window.
        Release,
        /// Setting turned off mid-press: stop unconditionally.
        Disable,
    }

    /// Poll interval while the hook is armed (keeps hook latency low) and
    /// while disarmed (cheap idle).
    const POLL_ARMED: Duration = Duration::from_millis(5);
    const POLL_DISARMED: Duration = Duration::from_millis(50);

    /// Grace window a release waits out for a possible re-press before it is
    /// allowed to stop the session. Middle-switch chatter (a contact that
    /// momentarily opens while the finger still holds the button, e.g. while
    /// scrolling the wheel) re-presses within a few milliseconds; the window
    /// leaves a wide margin over that. It must stay well under human
    /// re-click speed (≈200 ms and up) so two deliberate press/release pairs
    /// are never merged into one session, and it only *delays* the stop — the
    /// recording itself keeps running until the stop, so a short window costs
    /// the user a few tens of milliseconds before transcription starts.
    const RELEASE_BOUNCE_WINDOW: Duration = Duration::from_millis(120);

    /// Outcome of a release once its grace window has been waited out.
    #[derive(Debug, Copy, Clone, PartialEq, Eq)]
    enum ReleaseResolution {
        /// A press re-asserted the button inside the window: switch chatter,
        /// the session must survive.
        Bounce,
        /// No re-press: the button really left the user's finger, stop it.
        Stop,
    }

    /// Pure decision for a release after its grace window.
    ///
    /// `next_press_delay` is how long after the release a `WM_MBUTTONDOWN`
    /// arrived, or `None` when no press arrived before
    /// [`RELEASE_BOUNCE_WINDOW`] expired. A press inside the window proves the
    /// switch never really opened (scroll-vibration chatter) and must NOT stop
    /// the session; everything else — no press at all, or a press only after
    /// the window — is a genuine release that must stop. Chatter can only
    /// delay a stop, never veto it. Pure so the release semantics are
    /// unit-testable without a real hook or mouse (the old
    /// `GetAsyncKeyState`-based test read a sample the OS does not settle until
    /// after the hook callback, which is exactly how it passed while breaking
    /// on real hardware).
    fn release_after_bounce_window(next_press_delay: Option<Duration>) -> ReleaseResolution {
        match next_press_delay {
            Some(delay) if delay <= RELEASE_BOUNCE_WINDOW => ReleaseResolution::Bounce,
            _ => ReleaseResolution::Stop,
        }
    }

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
                MiddleEvent::Up => handle_deferred_release(&mut rx).await,
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
                        // (spurious release while scrolling) shows up as a
                        // "release absorbed as switch chatter" line followed by
                        // a "start rejected: Already recording" line for the
                        // re-press; a genuine release shows up as the
                        // release/stop line instead.
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

    /// Worker-side `WM_MBUTTONUP`, bounce-tolerant.
    ///
    /// A release is never acted on at once: the worker waits out
    /// [`RELEASE_BOUNCE_WINDOW`] for a possible re-press. A mechanical middle
    /// switch that is still held (the vibration of scrolling the wheel chatters
    /// the contacts open) makes Windows emit a real `WM_MBUTTONUP` immediately
    /// followed by a `WM_MBUTTONDOWN`; that press is the proof the button never
    /// left the user's finger, so the release is absorbed as chatter
    /// ([`release_after_bounce_window`]) and the session keeps running. No press
    /// inside the window means the release was genuine and the stop always
    /// proceeds, so chatter can only ever *delay* a stop — never cancel it (the
    /// P0 regression: the old `GetAsyncKeyState` veto could, and did, swallow
    /// every genuine release).
    ///
    /// Only the physical button decides this; `Disable` deliberately skips the
    /// window (it must always stop, even mid-press). The wait is safe on the
    /// serial worker: events stay queued in order and any `Disable`/press that
    /// does arrive is handled immediately by this same loop.
    async fn handle_deferred_release(rx: &mut UnboundedReceiver<MiddleEvent>) {
        let started = std::time::Instant::now();
        match tokio::time::timeout(RELEASE_BOUNCE_WINDOW, rx.recv()).await {
            // Re-press inside the window: chatter while the button is still
            // held. Absorb the release and treat the press normally (the
            // session is still `Recording`, so `start_session_internal`
            // rejects it as "Already recording" and the token is untouched).
            Ok(Some(MiddleEvent::Down { root_hwnd })) => {
                match release_after_bounce_window(Some(started.elapsed())) {
                    ReleaseResolution::Bounce => {
                        log::debug!("mouse: release absorbed as switch chatter while scrolling");
                        handle_middle_down(root_hwnd);
                    }
                    // Only reachable if the timer expired in the same instant
                    // the press was dequeued: the release had already matured,
                    // so it stops and the press is a fresh one.
                    ReleaseResolution::Stop => {
                        handle_middle_up(StopCause::Release);
                        handle_middle_down(root_hwnd);
                    }
                }
            }
            // The setting turned off inside the window must still stop at once.
            Ok(Some(MiddleEvent::Disable)) => handle_middle_up(StopCause::Disable),
            // Two releases with no press between them: the button is genuinely
            // up (chatter would have re-pressed first), so stop.
            Ok(Some(MiddleEvent::Up)) => handle_middle_up(StopCause::Release),
            // Channel closed (worker shutting down) or the window elapsed with
            // no re-press: the release is genuine.
            Ok(None) => {}
            Err(_elapsed) => handle_middle_up(StopCause::Release),
        }
    }

    /// Worker-side stop decision for a release past its grace window / for a
    /// disable: stop only the session this hook started and that is still
    /// recording; anything else is a no-op.
    fn handle_middle_up(cause: StopCause) {
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
                "mouse: middle-button {cause:?} ignored (owned session {owned_id} is no longer recording)"
            );
            return;
        }
        log::debug!("mouse: middle-button {cause:?} stopping session {owned_id}");
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
        /// `WM_MBUTTONUP`: end it, after waiting out the bounce grace window
        /// (see [`handle_deferred_release`] / [`release_after_bounce_window`]).
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

        /// Regression (bug ⑦, hardware bounce): a release whose button
        /// re-presses inside the grace window is switch chatter caused by
        /// scrolling with the middle button held — the session must survive,
        /// with its ownership token intact so the genuine release still stops
        /// it later.
        #[test]
        fn press_inside_the_window_is_bounce_chatter() {
            assert_eq!(
                release_after_bounce_window(Some(Duration::ZERO)),
                ReleaseResolution::Bounce
            );
            assert_eq!(
                release_after_bounce_window(Some(Duration::from_millis(1))),
                ReleaseResolution::Bounce
            );
            assert_eq!(
                release_after_bounce_window(Some(RELEASE_BOUNCE_WINDOW)),
                ReleaseResolution::Bounce,
                "the window boundary is inclusive"
            );
        }

        /// Regression (P0, bug 2): a genuine release must ALWAYS stop. No press
        /// inside the window — or a press only after it — is not chatter, so
        /// the stop must never be vetoed. The removed
        /// `GetAsyncKeyState`-based check vetoed every real release and left the
        /// recording running until the 5-minute cap.
        #[test]
        fn genuine_release_always_stops() {
            assert_eq!(release_after_bounce_window(None), ReleaseResolution::Stop);
            assert_eq!(
                release_after_bounce_window(Some(RELEASE_BOUNCE_WINDOW + Duration::from_millis(1))),
                ReleaseResolution::Stop
            );
            assert_eq!(
                release_after_bounce_window(Some(Duration::from_secs(5))),
                ReleaseResolution::Stop
            );
        }

        /// The window must be a real chatter filter yet stay well below human
        /// re-click speed, so a genuine release still stops promptly and two
        /// deliberate press/release pairs are never merged.
        #[test]
        fn bounce_window_is_tuned_between_chatter_and_human_speed() {
            assert!(RELEASE_BOUNCE_WINDOW >= Duration::from_millis(5));
            assert!(RELEASE_BOUNCE_WINDOW <= Duration::from_millis(200));
        }

        /// Full scroll-chatter timeline: while the user scrolls with the
        /// button held, the switch chatters (each Up/Down pair) and every Up
        /// must be absorbed; the final Up with no press after it must stop the
        /// session. Locks bug ⑦ and P0 (bug 2) together in one sequence.
        #[test]
        fn scroll_chatter_then_final_release_still_stops() {
            for chatter in [
                Duration::ZERO,
                Duration::from_millis(3),
                Duration::from_millis(15),
            ] {
                assert_eq!(
                    release_after_bounce_window(Some(chatter)),
                    ReleaseResolution::Bounce,
                    "chatter {chatter:?} after a release must not stop the session"
                );
            }
            // The finger finally lifts: no re-press ever arrives.
            assert_eq!(release_after_bounce_window(None), ReleaseResolution::Stop);
        }
    }
}
