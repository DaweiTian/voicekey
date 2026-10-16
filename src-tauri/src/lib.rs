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
