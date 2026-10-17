//! Window lifecycle helpers shared across the app.
//!
//! Two concerns live here:
//!
//! 1. **Windows focus-frame hardening.** Windows 11 draws a 1px DWM border
//!    around an activated borderless window. The HUD overlay must never get one
//!    (`WS_EX_NOACTIVATE` + `DWMWA_BORDER_COLOR=NONE`), while the main window
//!    only loses the system border (`DWMWA_BORDER_COLOR=NONE`) and stays
//!    focusable. Because DWM redraws the accent focus border on every
//!    activation, the suppression is re-asserted from `lib.rs`'s `Focused`
//!    handler, not only at window creation. The overlay *additionally* has its native caption / sizing-frame
//!    window bits stripped (see [`strip_native_frame`]): tao builds every window
//!    with `WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX` and only
//!    hides them by collapsing the client area - a hidden trap that makes Windows
//!    paint a native titlebar (minimize / restore / close buttons + a frame line
//!    above the capsule) once any non-client area exists. The overlay also has
//!    its Windows 11 DWM system backdrop / rounded-corner base cleared
//!    ([`clear_dwm_window_backdrop`]) so DWM cannot composite an opaque
//!    rectangle behind the capsule. Every helper is idempotent so it can be
//!    re-applied on each show without side effects.
//!
//!    **The capsules must also be non-focusable** (`focusable: false` in
//!    `tauri.conf.json` + `.focusable(false)` in the rebuild builders + the
//!    `set_focusable(false)` assertion in [`harden_overlay_window`]). tao
//!    rewrites the whole `GWL_EXSTYLE` word from its own computed styles on any
//!    flag change - and `show()` is such a change - carrying `WS_EX_NOACTIVATE`
//!    only when the window is *not* focusable. A focusable capsule therefore
//!    loses `WS_EX_NOACTIVATE` right before tao's `ShowWindow(SW_SHOW)` and gets
//!    **activated**, which is the documented trigger of the WebView2 ghost
//!    titlebar (tauri#14764). Non-focusable keeps the bit in tao's own styles,
//!    so no show can activate a capsule.
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
//! 4. **Session capsule rebuild.** The `session` window (the fixed
//!    bottom-center session capsule, decoupled from the draggable overlay in
//!    0.5.6) follows the same rule: [`ensure_session_window`] rebuilds it from
//!    the same shape as the `session` entry in `tauri.conf.json` if a close
//!    path ever really destroyed it, so a session start can always bring the
//!    capsule up. It gets the *same* Windows hardening as the overlay
//!    ([`harden_overlay_window`]): same geometry shape, same no-activate /
//!    no-frame / no-opaque-base requirements.
//! 5. **WebView2 transparent background.** Both capsules are `transparent: true`
//!    and the layer WebView2 paints *underneath* the page is white by default.
//!    Whenever the window's surface is re-established - the `show()` of a hidden
//!    capsule, or a bounds change such as the ghost-titlebar nudge - a white
//!    frame can be presented before the page content for that state exists.
//!    [`enable_transparent_webview_background`] removes that white source from
//!    process start (the documented cure for exactly this flicker).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use tauri::{AppHandle, Manager, WebviewWindow};

/// Label of the main (settings) window. A rebuilt window must keep this exact
/// label: `tauri.conf.json` and the renderer (`getCurrentWindow().label`) both
/// rely on it.
pub const MAIN_WINDOW_LABEL: &str = "main";

/// Label of the HUD overlay window.
pub const OVERLAY_WINDOW_LABEL: &str = "overlay";

/// Label of the session capsule window (fixed bottom-center session visuals,
/// decoupled from the draggable overlay capsule). Same rationale as
/// [`OVERLAY_WINDOW_LABEL`]: `tauri.conf.json` and the renderer
/// (`getCurrentWindow().label` → `App.tsx` routes it to `SessionHUD`) both
/// rely on this exact string.
pub const SESSION_WINDOW_LABEL: &str = "session";

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

/// Serializes session-window (re)creation, same invariant as
/// [`OVERLAY_CREATION_LOCK`]: [`ensure_session_window`] is only ever called
/// from worker threads (`commands::session_show`'s rebuild thread), never from
/// the main thread.
static SESSION_CREATION_LOCK: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------
// Windows focus-frame hardening
// ---------------------------------------------------------------------------

/// Ask DWM to suppress its window border entirely (Windows 11 build 22000+;
/// silently ignored on older systems). Idempotent: safe to call as often as we
/// like. Only the *system* border is suppressed; the app's own CSS keeps
/// rendering as usual.
///
/// **Must be re-asserted on every activation.** MSDN is explicit that the app
/// "is responsible for changing the border color according to state changes,
/// such as a change in window activation": Windows 11 redraws the accent-color
/// focus border the moment a window is activated, so the setup-time pass alone
/// does not survive the first focus. That is exactly the reported defect - a
/// blue focus ring appears on the main window the moment it gains focus - the
/// attribute was set at creation but never re-asserted. The `Focused` hook in
/// `lib.rs` re-runs the matching hardening pass on every focus change; keep it.
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
        // (`DWMWA_COLOR_DEFAULT` = 0xFFFF_FFFF would *reset* to the system
        // accent color, i.e. put the blue border back - do not use it here.)
        let hr = DwmSetWindowAttribute(
            hwnd_ptr,
            DWMWA_BORDER_COLOR as _,
            &DWMWA_COLOR_NONE as *const u32 as *const _,
            std::mem::size_of::<u32>() as u32,
        );
        if hr < 0 {
            // Older Windows (< build 22000) rejects this attribute; nothing to
            // do there, but log it so a failed suppression is never invisible.
            log::warn!("DWMWA_BORDER_COLOR (none) not applied (HRESULT {hr:#010x})");
        }
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

/// Remove the native caption / sizing-frame bits from the overlay window.
///
/// Why this exists: tao builds *every* window - decorated or not - with
/// `WS_CAPTION | WS_SYSMENU` plus `WS_MINIMIZEBOX | WS_MAXIMIZEBOX`
/// (minimizable / maximizable default to `true`; only `WS_SIZEBOX` follows
/// `resizable`), then relies on its `WM_NCCALCSIZE` handler collapsing the client
/// area to the whole window to keep them hidden. That only holds while the window
/// has **no** non-client area. tao's own `calculate_insets_for_dpi` carries the
/// warning verbatim: "if there is any nonclient area, Windows will draw a full
/// native titlebar outside the client area" - and that titlebar carries exactly
/// the minimize / restore / close buttons, plus the frame line above the client
/// area, that the transparent capsule must never show.
///
/// The default `shadow: true` is what creates that non-client area on Windows
/// (tao's `MARKER_UNDECORATED_SHADOW` path reserves frame-thickness insets around
/// the client area). The `overlay` entry disables it via `"shadow": false`, but
/// the caption bits are stripped here anyway so that *no* tao / DWM path can ever
/// paint a native titlebar on the capsule. Idempotent: it early-returns once the
/// bits are gone, so the per-show hardening pass stays free.
///
/// `SWP_FRAMECHANGED` is required to commit the change: without it Windows keeps
/// the cached (decorated) non-client area and keeps painting the frame.
#[cfg(target_os = "windows")]
fn strip_native_frame<R: tauri::Runtime>(window: &WebviewWindow<R>) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_STYLE, SWP_FRAMECHANGED,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_CAPTION, WS_MAXIMIZEBOX,
        WS_MINIMIZEBOX, WS_SYSMENU, WS_THICKFRAME,
    };

    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    let hwnd_ptr = hwnd.0;

    // The caption, the system menu it renders from, and the sizing-frame bits
    // that give a borderless window a native silhouette.
    const NATIVE_FRAME: u32 =
        WS_CAPTION | WS_SYSMENU | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX;

    unsafe {
        let style = GetWindowLongPtrW(hwnd_ptr, GWL_STYLE) as u32;
        let stripped = style & !NATIVE_FRAME;
        if stripped == style {
            return; // already clean: nothing to commit
        }
        SetWindowLongPtrW(hwnd_ptr, GWL_STYLE, stripped as isize);
        // Dropping the style bits alone is not enough: ask Windows to recompute
        // the non-client area and repaint the frame so the caption disappears now.
        SetWindowPos(
            hwnd_ptr,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// Remove every opaque *base* DWM can paint behind the capsule window.
///
/// Why this exists: the capsule must show nothing but its own WebView2 content,
/// yet on Windows 11 the frame-stripping above only removes the native
/// *non-client* frame. DWM still owns three independent, opaque-capable layers
/// that are **not** part of the non-client area and are therefore invisible to
/// `strip_native_frame`:
///
/// 1. `DWMWA_SYSTEMBACKDROP_TYPE` defaults to `DWMSBT_AUTO` (0) on Windows 11.
///    `AUTO` lets DWM pick a system backdrop for the window, and for the
///    borderless, always-on-top, `WS_POPUP` capsule it can resolve to a
///    translucent Acrylic surface (`DWMSBT_TRANSIENTWINDOW`). An Acrylic
///    backdrop is *sampled from what is composited underneath*, so it is the
///    one DWM layer whose appearance depends on the window behind the capsule -
///    exactly the reported signature: the capsule is clean over the desktop /
///    other apps but paints a white/gray rectangle **only while it floats over
///    our own (light) main window**. Forcing `DWMSBT_NONE` removes it.
/// 2. `DWMWA_WINDOW_CORNER_PREFERENCE` defaults to rounding the top corners on
///    Windows 11. Rounding clips DWM's base surface into view as a rounded
///    rectangle wherever the capsule is not, so it is pinned to `DONOTROUND`.
/// 3. `DWMWA_NCRENDERING_POLICY` is disabled so DWM never falls back to
///    rendering a non-client (opaque) base for the window.
///
/// Windows 11 22H2+ honors these attributes and silently ignores them on older
/// systems, so the call is safe everywhere. Idempotent: safe on every hardening
/// pass. Requires only the already-enabled `Win32_Graphics_Dwm` feature (the
/// `DWM_BLURBEHIND`/region APIs - which *would* need `Win32_Graphics_Gdi` and
/// cannot be re-asserted here - are deliberately not used; see the module
/// README for the follow-up option).
#[cfg(target_os = "windows")]
fn clear_dwm_window_backdrop<R: tauri::Runtime>(window: &WebviewWindow<R>) {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMNCRP_DISABLED, DWMSBT_NONE, DWMWA_NCRENDERING_POLICY,
        DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
    };

    let Ok(hwnd) = window.hwnd() else {
        return;
    };
    let hwnd_ptr = hwnd.0;
    // Every attribute below is a 32-bit integer value read by DWM from the
    // buffer; pass a pointer to a local of the attribute's own type (all are
    // i32-backed `DWMWINDOWATTRIBUTE` payloads) with its real size.
    let attr_size = std::mem::size_of::<u32>() as u32;
    unsafe {
        // Each attribute returns an `HRESULT` (`windows_sys::core::HRESULT` =
        // i32): 0 (S_OK) on success, negative on failure. Older Windows builds
        // silently reject the newer attributes (e.g. `DWMWA_SYSTEMBACKDROP_TYPE`
        // needs Win11 22H2+), so check the return and warn instead of assuming
        // success - a `let _ =` here hides exactly the case where an opaque base
        // survives and the capsule paints a rectangle again.

        // No DWM non-client rendering: no opaque base fill for the window.
        let nc_policy = DWMNCRP_DISABLED;
        let hr = DwmSetWindowAttribute(
            hwnd_ptr,
            DWMWA_NCRENDERING_POLICY as _,
            &nc_policy as *const _ as *const _,
            attr_size,
        );
        if hr < 0 {
            log::warn!("DWMWA_NCRENDERING_POLICY not applied (HRESULT {hr:#010x})");
        }

        // Never round the capsule: rounding would clip an opaque base into a
        // visible rounded rectangle around the capsule's transparent corners.
        let corner = DWMWCP_DONOTROUND;
        let hr = DwmSetWindowAttribute(
            hwnd_ptr,
            DWMWA_WINDOW_CORNER_PREFERENCE as _,
            &corner as *const _ as *const _,
            attr_size,
        );
        if hr < 0 {
            log::warn!("DWMWA_WINDOW_CORNER_PREFERENCE not applied (HRESULT {hr:#010x})");
        }

        // Force the system backdrop off (Mica / Acrylic / Tabbed). This is the
        // attribute that removes the sibling-window-dependent white/gray base.
        let backdrop = DWMSBT_NONE;
        let hr = DwmSetWindowAttribute(
            hwnd_ptr,
            DWMWA_SYSTEMBACKDROP_TYPE as _,
            &backdrop as *const _ as *const _,
            attr_size,
        );
        if hr < 0 {
            log::warn!("DWMWA_SYSTEMBACKDROP_TYPE not applied (HRESULT {hr:#010x})");
        }
    }
}

/// Clear the WebView2 "ghost titlebar" that WebView2 runtime 144+ paints over a
/// transparent window ([tauri#14764], upstream [WebView2Feedback#5492]).
///
/// Why this exists: after the native caption is stripped
/// ([`strip_native_frame`]) and DWM's opaque base is cleared
/// ([`clear_dwm_window_backdrop`]), a *third*, independent layer can still show
/// a light bar carrying the page `<title>` text above the capsule. That bar
/// follows the capsule while dragging and (re)appears on any activation / focus
/// change - opening the settings window, switching apps. It is **not** a window
/// style: `GWL_STYLE` already reports `WS_CAPTION` absent while the bar stays
/// painted, which is exactly why two rounds of style stripping could not remove
/// it. It is a WebView2 144.x regression, fixed upstream in Edge 146 / stable
/// but still live on unpatched runtimes.
///
/// The one reliable cure reported upstream is a **resize** (the issue records
/// that resizing the window makes the bar disappear, and our own symptom
/// matches: deforming the capsule - which resizes the window - clears it). So
/// nudge the window's height by one physical pixel and immediately back:
/// WebView2 repaints the host surface and drops the stale bar. Cheap and
/// idempotent.
///
/// The re-entrancy guard is essential: the nudge fires `Resized`, and any
/// `Resized` handler that itself nudged would loop forever (an upstream
/// workaround regressed the window off-screen that way). This runs at most once
/// at a time and is never called from a `Resized` handler.
///
/// [tauri#14764]: https://github.com/tauri-apps/tauri/issues/14764
/// [WebView2Feedback#5492]: https://github.com/MicrosoftEdge/WebView2Feedback/issues/5492
#[cfg(target_os = "windows")]
pub fn clear_webview_ghost_titlebar<R: tauri::Runtime>(window: &WebviewWindow<R>) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static NUDGING: AtomicBool = AtomicBool::new(false);
    if NUDGING.swap(true, Ordering::SeqCst) {
        return; // a nudge is already in flight (its Resized echo is ignored)
    }
    if let Ok(size) = window.inner_size() {
        let _ = window.set_size(tauri::PhysicalSize::new(size.width, size.height + 1));
        let _ = window.set_size(tauri::PhysicalSize::new(size.width, size.height));
    }
    NUDGING.store(false, Ordering::SeqCst);
}

/// Full hardening for the HUD overlay: it must never activate, never draw the
/// system focus frame, never show a native caption / frame (the three
/// minimize / restore / close buttons plus the window border), and never let
/// DWM paint an opaque system backdrop / rounded base behind it. Idempotent;
/// call before *and* after every show so a missed setup pass or a style reset
/// can never leave a visible frame or an opaque rectangle.
///
/// **0.5.8: the window is also asserted non-focusable, and that is the root fix
/// for the residual "white titlebar with 芷若" flash on click.** `prevent_activation`
/// below sets `WS_EX_NOACTIVATE` with `SetWindowLongPtrW`, but tao *rewrites the
/// whole style word* on any flag change: `WebviewWindow::show()` ->
/// `tao::WindowState::set_window_flags(VISIBLE)` -> `apply_diff`, which calls
/// `SetWindowLongW(GWL_EXSTYLE, to_window_styles().1)` and only *then*
/// `ShowWindow(SW_SHOW)`. tao's computed styles carry `WS_EX_NOACTIVATE` **only
/// when the window is not focusable** (`if !flags.contains(FOCUSABLE)`), and
/// tauri's `focusable` defaults to `true` - so the ex-style write **wiped our
/// `WS_EX_NOACTIVATE` and `ShowWindow(SW_SHOW)` was then free to activate the
/// capsule**. Activation is exactly the documented trigger of the WebView2 ghost
/// titlebar (tauri#14764: "drag or focus change"), whose text is the page
/// `<title>` - the white bar with 芷若 the user sees at the moment of the click
/// (that is the only moment a capsule is shown). 0.5.6 masked it with a
/// post-`show()` clearing nudge (the same nudge that produced the 0.5.7 white
/// *background* frame); 0.5.7 moved that nudge before the show, so the bar it
/// used to clear became visible again.
///
/// `set_focusable(false)` makes tao's own style computation keep
/// `WS_EX_NOACTIVATE` forever, so no `show()` can activate the capsule, so the
/// ghost is never triggered - no post-`show()` geometry change is needed (and
/// 0.5.7's white-flash fix stays intact). It is also the *correct* declaration
/// of what these capsules are: they are `WS_EX_NOACTIVATE` by design, they have
/// no keyboard interaction (mouse input is unaffected), and this closes the
/// latent focus-stealing bug where a session start could pull the foreground
/// away from the app the user is dictating into. The `focusable: false` entry in
/// `tauri.conf.json` (and `.focusable(false)` in the two rebuild builders) is
/// the declarative half; this assertion is the belt-and-braces that keeps the
/// invariant no matter how the window was created.
#[cfg(target_os = "windows")]
pub fn harden_overlay_window<R: tauri::Runtime>(window: &WebviewWindow<R>) {
    // Idempotent: a no-op once tao's FOCUSABLE flag is already false.
    let _ = window.set_focusable(false);
    prevent_activation(window);
    suppress_focus_frame(window);
    strip_native_frame(window);
    clear_dwm_window_backdrop(window);
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

/// Non-Windows stub: the WebView2 ghost-titlebar regression is Windows-only, so
/// there is nothing to clear elsewhere.
#[cfg(not(target_os = "windows"))]
pub fn clear_webview_ghost_titlebar<R: tauri::Runtime>(_window: &WebviewWindow<R>) {}

/// Ask the WebView2 runtime for a **fully transparent** default background
/// *before* it creates its environment, through the documented
/// `WEBVIEW2_DEFAULT_BACKGROUND_COLOR` environment variable.
///
/// Why this exists (0.5.7 white-flash fix): the capsule windows are
/// `transparent: true`, and the layer WebView2 paints *underneath* the page -
/// its `DefaultBackgroundColor` - is **white** by default (MSDN: "The default
/// value for this property is white"; it "renders this color when there is no
/// web content loaded such as before the initial navigation or between
/// navigations"). Every time the window's surface is (re)established the
/// runtime/DWM can present a frame before the page content for that state
/// exists, and what shows is that white default background. Two documented
/// triggers match the reported symptoms exactly:
///
/// * **`show()` of a hidden transparent window** - the session capsule is
///   `hide()`/`show()`n around every session (tauri#14515, "Creating a hidden
///   transparent window results in a white flash when displayed"; also
///   WebView2Feedback#5607: a white flash "when transforming the bounds of the
///   WebView or any other changes to it ... When showing it it's fine").
/// * **a bounds change** - our own ghost-titlebar nudge
///   ([`clear_webview_ghost_titlebar`]) resizes the window by 1px on every show
///   and focus change, which is a bounds change on a *visible* transparent
///   window: the flash side effect of the 0.5.6 white-bar workaround.
///
/// `wry` already sets `DefaultBackgroundColor` to transparent for
/// `transparent: true` windows, but it does so through the *property*
/// (`ICoreWebView2ControllerOptions3::SetDefaultBackgroundColor` at controller
/// creation, then the controller property again after). MSDN documents that
/// this is not enough to avoid the flash: "There is a known issue with
/// background color where just setting the color by property can still leave
/// the app with a white flicker before the `DefaultBackgroundColor` property
/// takes effect. Setting the color via environment variable solves this
/// issue." The environment variable is read once, when the WebView2
/// environment is created, so the transparent value is in force before the
/// first frame is ever composited.
///
/// Value format (MSDN): hex ARGB, optionally `0x`-prefixed; fewer than 8 digits
/// are treated as `00`-prefixed, i.e. transparent. `00000000` is fully
/// transparent. A value that is already present in the environment is left
/// untouched, so an operator/CI override always wins.
///
/// MUST run before any webview exists (the value is consumed when the WebView2
/// environment is created), which is why `lib.rs` calls it as the first
/// statement of `run()`.
#[cfg(target_os = "windows")]
pub fn enable_transparent_webview_background() {
    const VAR: &str = "WEBVIEW2_DEFAULT_BACKGROUND_COLOR";
    // Only set a default: never clobber an explicit override.
    if std::env::var_os(VAR).is_none() {
        std::env::set_var(VAR, "00000000");
    }
}

/// Non-Windows stub: `WEBVIEW2_DEFAULT_BACKGROUND_COLOR` is a WebView2 (Windows)
/// concept, and wry already keeps the other platforms' transparency intact.
#[cfg(not(target_os = "windows"))]
pub fn enable_transparent_webview_background() {}

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
            // The session capsule is the overlay's twin (same shape, same
            // no-activate / no-frame requirements), so it gets the same
            // hardening pass.
            OVERLAY_WINDOW_LABEL | SESSION_WINDOW_LABEL => harden_overlay_window(&window),
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
/// `tauri.conf.json` (280x64, decorations off, **shadow off**, transparent,
/// always-on-top, skip-taskbar, not initially focused, hidden until shown). Keep
/// the two in sync when the config changes. `shadow(false)` matters: on Windows
/// the default `true` gives an undecorated window a DWM frame (border + shadow +
/// rounded corners) drawn in a non-client band around the capsule - the extra
/// rectangle the capsule must not have.
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
    .shadow(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    // Keep in sync with the `overlay` entry in `tauri.conf.json`: the capsule
    // must be non-focusable so tao's own style writes keep `WS_EX_NOACTIVATE`
    // (see `harden_overlay_window` - a focusable window is activated by
    // `ShowWindow(SW_SHOW)` and WebView2 then paints its ghost titlebar).
    .focusable(false)
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

// ---------------------------------------------------------------------------
// Session capsule rebuild lifecycle
// ---------------------------------------------------------------------------

/// Get the session capsule window, rebuilding it on demand if a close path
/// destroyed it.
///
/// The session window (fixed bottom-center session visuals, shown by
/// `commands::session_show` on every session start) is normally only hidden
/// (`commands::session_hide`), and the `CloseRequested` handler in `lib.rs`
/// converts a renderer close into a hidden window. The rebuild mirrors
/// [`ensure_overlay_window`]: if a close path ever really destroyed the
/// window, `AppHandle::get_webview_window` would silently return `None` and
/// every session start would silently lose its capsule — rebuilding here keeps
/// "a session start can always bring the capsule up".
///
/// Threading: only call from worker threads - see [`SESSION_CREATION_LOCK`].
pub fn ensure_session_window<R: tauri::Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    let _guard = SESSION_CREATION_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(window) = app.get_webview_window(SESSION_WINDOW_LABEL) {
        harden_overlay_window(&window);
        return Some(window);
    }
    match build_session_window(app) {
        Ok(window) => Some(window),
        Err(e) => {
            // A concurrent rebuild won the race (duplicate label) or creation
            // failed transiently: prefer the existing window over giving up.
            log::warn!("session window build failed ({e}); falling back to the existing window");
            let window = app.get_webview_window(SESSION_WINDOW_LABEL)?;
            harden_overlay_window(&window);
            Some(window)
        }
    }
}

/// Rebuild the session capsule with the same shape as the `session` entry in
/// `tauri.conf.json` (280x64, decorations off, **shadow off**, transparent,
/// always-on-top, skip-taskbar, not initially focused, hidden until shown).
/// Keep the three in sync when the config changes - the shape is identical to
/// [`build_overlay_window`] on purpose: the session capsule reuses the
/// overlay's whole Windows hardening stack (no-activate, no DWM frame, no
/// opaque backdrop) and its rendering surface.
fn build_session_window<R: tauri::Runtime>(app: &AppHandle<R>) -> tauri::Result<WebviewWindow<R>> {
    let window = tauri::WebviewWindowBuilder::new(
        app,
        SESSION_WINDOW_LABEL,
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title("芷若")
    .inner_size(280.0, 64.0)
    .resizable(false)
    .decorations(false)
    .shadow(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    // Same as `build_overlay_window`: non-focusable, so tao's style writes keep
    // `WS_EX_NOACTIVATE` and `show()` cannot activate the capsule (activating it
    // is what makes WebView2 paint its ghost titlebar). Keep in sync with the
    // `session` entry in `tauri.conf.json`.
    .focusable(false)
    .visible(false)
    .build()?;

    // Same CloseRequested -> hide behavior as the config-created window (the
    // global `Builder::on_window_event` handler in `lib.rs` may already cover
    // this rebuilt window; registering here is idempotent belt-and-braces,
    // exactly like `build_overlay_window` above). Note this only *hides* — it
    // must never touch the overlay's persisted visibility flag.
    let handle = window.app_handle().clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            crate::commands::hide_session(&handle);
        }
    });

    // Never leave a freshly built session capsule without its hardening.
    harden_overlay_window(&window);
    Ok(window)
}
