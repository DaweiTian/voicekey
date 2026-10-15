mod asr;
mod audio;
mod commands;
mod config;
mod history;
mod hotkey;
mod inject;
mod llm;
mod platform;
mod state;
mod tray;
mod updater;

use state::AppState;
use tauri::Manager;

pub fn run() {
    env_logger::init();

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let state = AppState::new(app.handle())?;
            app.manage(state);

            if let Some(main) = app.get_webview_window("main") {
                let _ = main.set_title("Voice Key");
            }

            tray::setup_tray(app.handle())?;
            hotkey::bind_default_hotkeys(app.handle())?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::set_config,
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
            commands::show_settings_window,
            commands::refine_text,
            commands::translate_text,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Voice Key");
}
