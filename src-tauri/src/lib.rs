mod asr;
mod audio;
mod commands;
mod config;
mod history;
mod hotkey;
mod inject;
mod llm;
mod mouse;
mod platform;
mod state;
mod stats;
mod translate;
mod tray;
mod updater;
mod windowing;

use state::AppState;
use tauri::Manager;

/// Clear the WebView2 "ghost titlebar" on a capsule window (overlay or
/// session) after a focus / activation change
/// ([`windowing::clear_webview_ghost_titlebar`], tauri#14764). Called from
/// every window's `Focused` event: the reported triggers are opening the
/// settings page and switching windows, both of which change activation.
///
/// Skipped while the target capsule is hidden - the stale bar can only be
/// visible when the window is on screen, so repainting it otherwise is wasted
/// work (and would resize a hidden window on every app activation).
fn clear_capsule_ghost_titlebars<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    for label in [
        windowing::OVERLAY_WINDOW_LABEL,
        windowing::SESSION_WINDOW_LABEL,
    ] {
        if let Some(win) = app.get_webview_window(label) {
            if win.is_visible().unwrap_or(false) {
                windowing::clear_webview_ghost_titlebar(&win);
            }
        }
    }
}

pub fn run() {
    env_logger::init();

    tauri::Builder::default()
        // Single-instance guard, registered FIRST: a second launch hands its
        // argv to the running instance and exits before any other plugin or
        // `setup` work can double-run (no second tray icon / overlay / hotkey
        // grab / stats-store race). The callback runs in the FIRST instance.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // Show and focus the running instance's main window, rebuilding it
            // on a worker thread if the delayed destroy reclaimed it.
            windowing::open_main_window(app);
        }))
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        // Launch-at-login; toggled from the tray menu's checkable item (tray.rs).
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .on_window_event(|window, event| {
            // A focus / activation change is what makes WebView2 runtime 144+
            // paint its "ghost titlebar" over a transparent window
            // (tauri#14764); the reported triggers are opening the settings page
            // and switching windows. Clear it (a guarded 1px resize) on every
            // focus change of either window, skipped while the capsule is
            // hidden - the stale bar only exists on screen.
            if let tauri::WindowEvent::Focused(_) = event {
                // Re-assert the DWM border suppression on activation: Windows 11
                // (build 22000+) redraws its accent-color focus border the
                // moment a window is activated unless the app re-sets
                // DWMWA_BORDER_COLOR (MSDN: the app "is responsible for changing
                // the border color according to state changes, such as a change
                // in window activation"). The setup-time pass alone does not
                // survive the first activation - that is why the main window
                // still showed a blue focus ring. Both passes are idempotent.
                let label = window.label();
                let app = window.app_handle();
                if let Some(win) = app.get_webview_window(label) {
                    if label == windowing::MAIN_WINDOW_LABEL {
                        windowing::harden_main_window(&win);
                    } else if label == windowing::OVERLAY_WINDOW_LABEL
                        || label == windowing::SESSION_WINDOW_LABEL
                    {
                        // The session capsule shares the overlay's shape and
                        // no-activate hardening; without this branch it would
                        // re-draw the blue DWM focus border on activation,
                        // exactly like the overlay bug before it.
                        windowing::harden_overlay_window(&win);
                    }
                }
                // A focus change is also what (re)triggers the WebView2 ghost
                // titlebar over the transparent capsule (tauri#14764).
                clear_capsule_ghost_titlebars(app);
            }
            // Closing the main window hides it to the tray and arms a 5-minute
            // delayed destroy of its WebView (`windowing`); reopening rebuilds
            // the window on demand. The tray "Quit" item (`app.exit`) remains
            // the only real exit path.
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    windowing::hide_main_window_to_tray(window.app_handle());
                }
            } else if window.label() == windowing::OVERLAY_WINDOW_LABEL {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    // The HUD capsule must never be destroyed (the renderer
                    // holds `core:window:allow-close`): convert a close into
                    // hide + persist, so the tray "Show floating capsule"
                    // entry can always bring it back.
                    api.prevent_close();
                    commands::hide_overlay(window.app_handle());
                }
            } else if window.label() == windowing::SESSION_WINDOW_LABEL {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    // Same rule as the overlay: never destroy the session
                    // capsule. Convert a close into a plain hide — and only a
                    // hide: this must NOT touch the overlay's persisted
                    // visibility flag (`hide_session` is flag-free by
                    // construction, `hide_overlay` would not be).
                    api.prevent_close();
                    commands::hide_session(window.app_handle());
                }
            }
        })
        .setup(|app| {
            let state = AppState::new(app.handle())?;
            app.manage(state);

            if let Some(main) = app.get_webview_window("main") {
                let _ = main.set_title("芷若");
            }

            tray::setup_tray(app.handle())?;
            hotkey::bind_default_hotkeys(app.handle())?;
            // Best-effort first pass of the Windows focus-frame hardening (main
            // window: border only; overlay: no-activate + no border). Anything
            // missing here is covered by the per-show re-application in
            // `overlay_show` / `windowing::ensure_main_window`.
            windowing::harden_app_windows(app.handle());
            // Middle mouse button push-to-talk (Windows only; no-op elsewhere).
            // The hook installs / removes itself as the setting flips.
            mouse::init(app.handle());
            mouse::set_middle_click_ptt(
                app.state::<AppState>().config.load().mouse.middle_click_ptt,
            );
            // The HUD capsule is an always-on standby launcher now. The show
            // path re-applies the Windows hardening around `show()` and honors
            // the tray "hide floating capsule" toggle.
            commands::show_overlay_idle(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::set_config,
            commands::set_hotkey_capture_active,
            commands::get_config_secret,
            commands::test_refine_connection,
            commands::connect_token_dance,
            commands::get_local_asr_status,
            commands::download_local_asr,
            commands::delete_local_asr,
            commands::get_streaming_asr_status,
            commands::download_streaming_asr,
            commands::delete_streaming_asr,
            commands::open_asr_model_directory,
            commands::get_host_capabilities,
            commands::get_app_language,
            commands::set_app_language,
            commands::get_history,
            commands::get_stats,
            commands::clear_history,
            commands::delete_history_item,
            commands::start_session,
            commands::stop_session,
            commands::cancel_session,
            commands::get_session_status,
            commands::send_audio_chunk,
            commands::send_streaming_audio_frame,
            commands::send_streaming_audio_end,
            commands::send_error,
            commands::send_audio_level,
            commands::check_for_updates,
            commands::get_update_status,
            commands::get_app_version,
            commands::open_external,
            commands::get_log_tail,
            commands::open_log_folder,
            commands::log,
            commands::get_linux_integration_status,
            commands::install_linux_integration,
            commands::remove_linux_integration,
            commands::set_ignore_mouse_events,
            commands::set_overlay_size,
            commands::set_overlay_position,
            commands::show_settings_window,
            commands::refine_text,
            commands::translate_text,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Zhiruo");
}
