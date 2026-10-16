//! Single programmatic tray icon (with menu). Keep this as the *only* tray
//! source: the former `app.trayIcon` block in `tauri.conf.json` created a
//! second, menu-less tray icon, which showed up as a duplicate (blank) entry
//! in the Windows taskbar overflow.

use std::sync::Mutex;

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_autostart::ManagerExt;

/// Setter for the "Show floating capsule" checkmark, registered by
/// [`setup_tray`] in type-erased form so capsule visibility changes that
/// happen outside the tray (e.g. closing the capsule window directly) can
/// still keep the menu state in sync through [`set_overlay_menu_checked`].
static OVERLAY_CHECK_SETTER: Mutex<Option<Box<dyn Fn(bool) + Send + Sync>>> = Mutex::new(None);

/// Keep the tray "Show floating capsule" checkmark in sync with the capsule's
/// persisted visibility (`commands::sync_overlay_visibility_flag`).
pub fn set_overlay_menu_checked(checked: bool) {
    let setter = OVERLAY_CHECK_SETTER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(set) = setter.as_ref() {
        set(checked);
    }
}

/// Toggle launch-at-login from the tray menu and refresh the checkmark from
/// the plugin's real state so the menu never drifts from the OS setting.
/// Pure Rust side: no renderer round-trip and therefore no capability
/// permission required.
fn toggle_autostart<R: Runtime>(app: &AppHandle<R>, item: &CheckMenuItem<R>) {
    let autolaunch = app.autolaunch();
    let result = match autolaunch.is_enabled() {
        Ok(true) => autolaunch.disable(),
        Ok(false) => autolaunch.enable(),
        // Unknown current state: prefer enabling so the menu item and the OS
        // setting converge on a definite value.
        Err(e) => {
            log::warn!("autostart: failed to read current state: {e}");
            autolaunch.enable()
        }
    };
    match result {
        Ok(()) => {
            let enabled = autolaunch.is_enabled().unwrap_or(true);
            // Windows menus auto-toggle their checkmark on click, macOS does
            // not; always set it explicitly so both platforms stay in sync.
            let _ = item.set_checked(enabled);
        }
        Err(e) => log::error!("autostart: failed to update launch-at-login: {e}"),
    }
}

pub fn setup_tray<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let show = MenuItem::with_id(app, "show", "Open Zhiruo", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let separator_top = PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?;
    // Checkable item; the initial state mirrors the persisted OS setting.
    let autostart_enabled = app.autolaunch().is_enabled().unwrap_or(false);
    let autostart = CheckMenuItem::with_id(
        app,
        "autostart",
        "Launch at login",
        true,
        autostart_enabled,
        None::<&str>,
    )
    .map_err(|e| e.to_string())?;
    // Kept alive by the menu-event closure so the toggle can update its
    // checkmark (TrayIcon has no `menu()` getter in this tauri version).
    let autostart_item = autostart.clone();
    // Checkable item for the always-on HUD capsule. Checked = visible; hiding
    // it never affects the PTT hotkey or the middle-click trigger. The state
    // lives in `app.overlayVisible` (config) so it survives restarts.
    let overlay_visible = app
        .try_state::<crate::state::AppState>()
        .map(|s| s.config.load().app.overlay_visible)
        .unwrap_or(true);
    let overlay_toggle = CheckMenuItem::with_id(
        app,
        "overlay-toggle",
        "Show floating capsule",
        true,
        overlay_visible,
        None::<&str>,
    )
    .map_err(|e| e.to_string())?;
    // Register the checkmark sync so visibility changes outside the tray
    // (e.g. closing the capsule window) keep this menu item in step.
    let overlay_toggle_sync = overlay_toggle.clone();
    *OVERLAY_CHECK_SETTER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Box::new(move |checked| {
        // Never fail silently: a stuck checkmark would make the tray entry
        // lie about the capsule state and is hard to diagnose otherwise.
        if let Err(e) = overlay_toggle_sync.set_checked(checked) {
            log::warn!("tray: floating-capsule checkmark sync failed: {e}");
        }
    }));
    let separator_bottom = PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?;
    let quit =
        MenuItem::with_id(app, "quit", "Quit", true, None::<&str>).map_err(|e| e.to_string())?;

    let menu = Menu::with_items(
        app,
        &[
            &show,
            &settings,
            &separator_top,
            &autostart,
            &overlay_toggle,
            &separator_bottom,
            &quit,
        ],
    )
    .map_err(|e| e.to_string())?;

    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| "Missing tray icon".to_string())?;

    TrayIconBuilder::with_id("main-tray")
        .icon(icon)
        .tooltip("芷若")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "show" | "settings" => {
                // Rebuilds the main window if the delayed destroy reclaimed it.
                crate::windowing::open_main_window(app);
            }
            "autostart" => toggle_autostart(app, &autostart_item),
            "overlay-toggle" => {
                // Decide from the capsule's *actual* window visibility, never
                // from the persisted flag: the two can drift when another path
                // (e.g. closing the capsule window from the renderer) hides or
                // destroys it without writing the flag, and this entry must
                // always be able to bring the capsule back. The show/hide
                // helpers re-sync `app.overlayVisible` and the checkmark to
                // the state actually reached (Windows menus auto-toggle their
                // checkmark on click, macOS does not; setting it explicitly
                // keeps both in step).
                let visible = app
                    .get_webview_window(crate::windowing::OVERLAY_WINDOW_LABEL)
                    .and_then(|w| w.is_visible().ok())
                    .unwrap_or(false);
                if visible {
                    crate::commands::hide_overlay(app);
                } else {
                    crate::commands::show_overlay_forced(app);
                }
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                // Rebuilds the main window if the delayed destroy reclaimed it.
                crate::windowing::open_main_window(app);
            }
        })
        .build(app)
        .map_err(|e| e.to_string())?;

    Ok(())
}
