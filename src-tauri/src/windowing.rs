//! Window lifecycle helpers shared across the app.
//!
//! Two concerns live here:
//!
//! 1. **Windows focus-frame hardening.** Windows 11 draws a 1px DWM border
//!    around an activated borderless window. The HUD overlay must never get one
//!    (`WS_EX_NOACTIVATE` + `DWMWA_BORDER_COLOR=NONE`), while the main window
//!    only loses the system border (`DWMWA_BORDER_COLOR=NONE`) and stays
//!    focusable. Every helper is idempotent so it can be re-applied on each
//!    show without side effects.
//! 2. **Main window hide-to-tray lifecycle.** Closing the main window hides it
//!    to the tray and arms a [`MAIN_DESTROY_DELAY`] timer that destroys the
//!    whole main WebView process to reclaim memory. Reopening (tray menu /
//!    settings hotkey / IPC) rebuilds it on demand through
//!    [`ensure_main_window`] with the same configuration.
//! 3. **HUD overlay rebuild.** The capsule window is normally only hidden and
//!    shown (`commands::overlay_show` / `hide_overlay`), but if it ever gets
//!    destroyed (a renderer close call, `core:window:allow-close`, before the
//!    CloseRequested handler existed) [`ensure_overlay_window`] rebuilds it
//!    from the same shape as the `overlay` entry in `tauri.conf.json`, so the
//!    tray "Show floating capsule" entry can always bring it back.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use tauri::{AppHandle, Manager, WebviewWindow};

/// Label of the main (settings) window. A rebuilt window must keep this exact
/// label: `tauri.conf.json` and the renderer (`getCurrentWindow().label`) both
/// rely on it.
pub const MAIN_WINDOW_LABEL: &str = "main";

/// Label of the HUD overlay window.
pub const OVERLAY_WINDOW_LABEL: &str = "overlay";

/// How long a hidden main window survives before its WebView process is
/// destroyed to reclaim memory (the window itself already hid to the tray).
///
/// Config persistence is safe well within this budget: the frontend's only
/// delayed write is the 700ms debounced auto-save (`AUTO_SAVE_DELAY_MS` in
/// `src/pages/SettingsPage.tsx`), which flushes through the synchronous
/// `set_config` -> `ConfigStore::merge` path (file write goes through the
/// lock-held `save_unlocked`, whole call under `CONFIG_LOCK`) as soon as the
/// debounce elapses while the hidden window is still alive. There are no other
/// pending-save paths (every remaining write saves inline). Five minutes is far
/// beyond that, so the destroy can never drop a pending save.
pub const MAIN_DESTROY_DELAY: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Generation counter for the delayed-destroy timer. Every
/// [`schedule_main_window_destroy`] records the generation its timer must still
/// observe when it fires; any newer schedule or reopen bumps the counter so
/// stale timers give up. This is what makes "hide -> reopen within 5 minutes"
/// safe: the old timer can never destroy the freshly reopened window.
static DESTROY_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Serializes main-window (re)creation so two overlapping opens can never
/// create two windows with the same label.
///
/// INVARIANT: [`ensure_main_window`] is only ever called from worker threads
/// ([`open_main_window`]'s dedicated thread or an `async` command), never from
/// the main thread. It blocks on window creation, and on Windows the main thread
/// must stay free to process the creation message (wry#583: creating a webview
/// window while blocking the main thread / inside a WebView2 callback
/// deadlocks). Holding this lock is then always safe: no main-thread code can
/// wait on it while a holder waits for the main thread.
static MAIN_CREATION_LOCK: Mutex<()> = Mutex::new(());

/// Serializes overlay-window (re)creation for the same reason and with the
/// same invariant as [`MAIN_CREATION_LOCK`]: [`ensure_overlay_window`] is only
/// ever called from worker threads (`commands::overlay_show`'s rebuild thread),
/// never from the main thread, because window creation blocks on the main
/// thread's creation message (wry#583).
static OVERLAY_CREATION_LOCK: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------
// Windows focus-frame hardening
// ---------------------------------------------------------------------------

/// Ask DWM to skip its 1px window border entirely (Windows 11 22H2+; silently
/// ignored on older systems). Idempotent: safe to call as often as we like.
/// Only the *system* frame is suppressed; the app's own CSS border keeps
/// rendering as usual.
#[cfg(target_os = "windows")]
fn suppress_focus_frame<R: tauri::Runtime>(window: &WebviewWindow<R>) {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE,
    };

    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    // Tauri hands back the `windows` crate's `HWND` tuple struct; unwrap it to
    // the raw pointer that `windows-sys` expects.
    let hwnd_ptr = hwnd.0;
    unsafe {
        // `DWMWA_COLOR_NONE` (= 0xFFFF_FFFE, "no border color at all", see
        // MSDN DWMWINDOWATTRIBUTE) IS exported by windows-sys 0.59
        // (`Win32::Graphics::Dwm::DWMWA_COLOR_NONE`), so use the real constant.
        // REGRESSION WARNING: a hand-rolled literal of 0x0100_0000 was used
        // here once; DWM interpreted it as a COLORREF and painted the border
        // near-black instead of removing it, silently defeating the fix.
        let _ = DwmSetWindowAttribute(
            hwnd_ptr,
            DWMWA_BORDER_COLOR as _,
            &DWMWA_COLOR_NONE as *const u32 as *const _,
            std::mem::size_of::<u32>() as u32,
        );
    }
}

/// Make the window incapable of becoming the foreground/active window, so the
/// DWM focus frame is never drawn around it. Idempotent.
///
/// Why this is needed: the `focus: false` window option only affects the
/// *initial* show (`SW_SHOWNOACTIVATE`); every later `WebviewWindow::show()`
/// maps to `ShowWindow(SW_SHOW)`, which activates the window, and Windows 11
/// draws its 1px focus border around an activated borderless window. With
/// `WS_EX_NOACTIVATE` the window never activates, so no frame ever appears.
/// Mouse input (the HUD's cancel button) is unaffected.
#[cfg(target_os = "windows")]
fn prevent_activation<R: tauri::Runtime>(window: &WebviewWindow<R>) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE,
    };

    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    let hwnd_ptr = hwnd.0;
    unsafe {
        let ex_style = GetWindowLongPtrW(hwnd_ptr, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd_ptr, GWL_EXSTYLE, ex_style | WS_EX_NOACTIVATE as isize);
    }
}

/// Full hardening for the HUD overlay: it must never activate and never draw
/// the system focus frame. Idempotent; call before *and* after every show so a
/// missed setup pass or a style reset can never leave a visible frame.
#[cfg(target_os = "windows")]
pub fn harden_overlay_window<R: tauri::Runtime>(window: &WebviewWindow<R>) {
    prevent_activation(window);
    suppress_focus_frame(window);
}

/// Main-window hardening: strip only the DWM focus frame. The main window must
/// stay focusable and interactive, so `WS_EX_NOACTIVATE` is deliberately *not*
/// applied here. Idempotent; applied on creation and on every open path.
#[cfg(target_os = "windows")]
pub fn harden_main_window<R: tauri::Runtime>(window: &WebviewWindow<R>) {
    suppress_focus_frame(window);
}

#[cfg(not(target_os = "windows"))]
pub fn harden_overlay_window<R: tauri::Runtime>(_window: &WebviewWindow<R>) {}

#[cfg(not(target_os = "windows"))]
pub fn harden_main_window<R: tauri::Runtime>(_window: &WebviewWindow<R>) {}

/// Apply the per-window hardening to every window that exists right now.
///
/// Called from `setup` as the best-effort first pass. If a window (typically
/// the overlay) is not there yet at that point nothing is lost: the hardening
/// is re-applied on every show path (`overlay_show`, [`ensure_main_window`]),
/// which is the authoritative application point.
pub fn harden_app_windows<R: tauri::Runtime>(app: &AppHandle<R>) {
    for (label, window) in app.webview_windows() {
        match label.as_str() {
            MAIN_WINDOW_LABEL => harden_main_window(&window),
            OVERLAY_WINDOW_LABEL => harden_overlay_window(&window),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Main window hide-to-tray / delayed-destroy lifecycle
// ---------------------------------------------------------------------------

/// Cancel any pending delayed destroy of the main window (it is being reopened
/// or the app is about to do something else with it).
pub fn cancel_main_window_destroy() {
    DESTROY_GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// Hide the main window to the tray and arm the delayed destroy of its WebView.
/// Shared by every CloseRequested path (`lib.rs`'s global handler and the
/// rebuilt window's own listener); idempotent, so double registration is safe.
pub fn hide_main_window_to_tray<R: tauri::Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = window.hide();
    }
    schedule_main_window_destroy(app);
}

/// Arm (or re-arm) the delayed destroy of the hidden main window.
///
/// Only the newest timer acts: the generation captured here must still be the
/// current one when the timer fires, so "hide -> reopen within 5 minutes" can
/// never be destroyed by the older timer (and neither can a hide -> reopen ->
/// hide sequence get torn down early).
pub fn schedule_main_window_destroy<R: tauri::Runtime>(app: &AppHandle<R>) {
    let generation = DESTROY_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(MAIN_DESTROY_DELAY).await;
        // Run the final checks and the destroy on the main thread: `destroy`
        // must not be invoked from a WebView2 callback, and re-checking the
        // generation at execution time closes the race with a concurrent
        // reopen (`ensure_main_window` bumps the generation before handing the
        // window out).
        let checker = handle.clone();
        let result = handle.run_on_main_thread(move || {
            if DESTROY_GENERATION.load(Ordering::SeqCst) != generation {
                return; // stale timer: the window was reopened in the meantime
            }
            let Some(window) = checker.get_webview_window(MAIN_WINDOW_LABEL) else {
                return;
            };
            // Belt and braces: never tear down a window that is on screen.
            if window.is_visible().unwrap_or(false) {
                return;
            }
            // `destroy` (unlike `close`) force-closes without emitting another
            // CloseRequested, so no hide/destroy loop can form. The tray Quit
            // path stays the only real exit; process exit reclaims whatever is
            // still pending.
            if let Err(e) = window.destroy() {
                log::warn!("main window destroy failed: {e}");
            }
        });
        if let Err(e) = result {
            log::warn!("main window destroy dispatch failed: {e}");
        }
    });
}

/// Get the main window, rebuilding it on demand after a delayed destroy.
///
/// Every "open the main window" path must reach this function (via
/// [`open_main_window`]) instead of `AppHandle::get_webview_window("main")`,
/// which silently returns `None` once the window has been destroyed. The
/// rebuilt window keeps the exact shape of the `main` entry in
/// `tauri.conf.json` and the `"main"` label the renderer relies on.
///
/// Also cancels any pending delayed destroy (before anything else, so a timer
/// firing mid-rebuild sees the bumped generation and gives up) and re-applies
/// the Windows focus-frame hardening so every show/set_focus path is covered.
///
/// Threading: only call from worker threads - see [`MAIN_CREATION_LOCK`].
pub fn ensure_main_window<R: tauri::Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    cancel_main_window_destroy();
    let _guard = MAIN_CREATION_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        harden_main_window(&window);
        return Some(window);
    }
    match build_main_window(app) {
        Ok(window) => Some(window),
        Err(e) => {
            // A concurrent open won the race (duplicate label) or creation
            // failed transiently: prefer the winner's window over giving up.
            log::warn!("main window build failed ({e}); falling back to the existing window");
            let window = app.get_webview_window(MAIN_WINDOW_LABEL)?;
            harden_main_window(&window);
            Some(window)
        }
    }
}

/// Show (and focus) the main window, rebuilding it if the delayed destroy
/// already reclaimed it. Safe to call from any event handler or command: the
/// get-or-create runs on a dedicated worker thread so the main thread is always
/// free to process the window-creation message (creating a webview window while
/// blocking the main thread deadlocks on Windows, wry#583).
pub fn open_main_window<R: tauri::Runtime>(app: &AppHandle<R>) {
    let handle = app.clone();
    std::thread::spawn(move || match ensure_main_window(&handle) {
        Some(window) => {
            let _ = window.show();
            let _ = window.set_focus();
        }
        None => log::error!("open_main_window: main window is unavailable"),
    });
}

/// Rebuild the main window with the same shape as the `main` entry in
/// `tauri.conf.json` (1100x720, min 600x500, `decorations: false`, centered).
/// Keep the two in sync when the config changes.
fn build_main_window<R: tauri::Runtime>(app: &AppHandle<R>) -> tauri::Result<WebviewWindow<R>> {
    let window = tauri::WebviewWindowBuilder::new(
        app,
        MAIN_WINDOW_LABEL,
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title("芷若")
    .inner_size(1100.0, 720.0)
    .min_inner_size(600.0, 500.0)
    .resizable(true)
    .fullscreen(false)
    .center()
    .decorations(false)
    .build()?;

    // `Builder::on_window_event` in `lib.rs` covers the windows created from
    // `tauri.conf.json`; register the same CloseRequested -> hide + delayed
    // destroy behavior explicitly for this rebuilt window so its close button
    // can never really close it. (Both handlers firing would be harmless:
    // hide and the generation-based schedule are idempotent.)
    let handle = window.app_handle().clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            hide_main_window_to_tray(&handle);
        }
    });

    // Never leave a freshly built window with the system focus frame.
    harden_main_window(&window);
    Ok(window)
}

// ---------------------------------------------------------------------------
// HUD overlay rebuild lifecycle
// ---------------------------------------------------------------------------

/// Get the HUD overlay window, rebuilding it on demand if a close path
/// destroyed it.
///
/// The capsule is normally only hidden (`commands::overlay_hide`), and the
/// `CloseRequested` handler in `lib.rs` converts a renderer close into a
/// hidden window. But any path that really destroys the overlay (e.g. a
/// `close()` racing the handler on older builds) would otherwise make the
/// capsule permanently irrecoverable: `AppHandle::get_webview_window` silently
/// returns `None` and every show path becomes a no-op. Rebuilding here keeps
/// the guarantee "the tray can always bring the capsule back".
///
/// Threading: only call from worker threads - see [`OVERLAY_CREATION_LOCK`].
pub fn ensure_overlay_window<R: tauri::Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    let _guard = OVERLAY_CREATION_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(window) = app.get_webview_window(OVERLAY_WINDOW_LABEL) {
        harden_overlay_window(&window);
        return Some(window);
    }
    match build_overlay_window(app) {
        Ok(window) => Some(window),
        Err(e) => {
            // A concurrent rebuild won the race (duplicate label) or creation
            // failed transiently: prefer the existing window over giving up.
            log::warn!("overlay window build failed ({e}); falling back to the existing window");
            let window = app.get_webview_window(OVERLAY_WINDOW_LABEL)?;
            harden_overlay_window(&window);
            Some(window)
        }
    }
}

/// Rebuild the HUD overlay with the same shape as the `overlay` entry in
/// `tauri.conf.json` (280x64, transparent, always-on-top, skip-taskbar, not
/// initially focused, hidden until shown). Keep the two in sync when the
/// config changes.
fn build_overlay_window<R: tauri::Runtime>(app: &AppHandle<R>) -> tauri::Result<WebviewWindow<R>> {
    let window = tauri::WebviewWindowBuilder::new(
        app,
        OVERLAY_WINDOW_LABEL,
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title("芷若")
    .inner_size(280.0, 64.0)
    .resizable(false)
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    .visible(false)
    .build()?;

    // Same CloseRequested -> hide behavior as the config-created window (the
    // global `Builder::on_window_event` handler in `lib.rs` may already cover
    // this rebuilt window; registering here is idempotent belt-and-braces,
    // exactly like `build_main_window` above - hide and the flag sync are
    // repeatable without side effects).
    let handle = window.app_handle().clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            crate::commands::hide_overlay(&handle);
        }
    });

    // Never leave a freshly built overlay without its no-activate hardening.
    harden_overlay_window(&window);
    Ok(window)
}
