//! piper-tauri — Tauri desktop app wrapping piper-core session API.

mod commands;

use std::sync::Arc;
use tokio::sync::Mutex;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_shell::init())
        .manage(Arc::new(Mutex::new(commands::SessionState::default())))
        .invoke_handler(tauri::generate_handler![
            commands::create_session,
            commands::join_session,
            commands::send_chat,
            commands::share_file,
            commands::start_download,
            commands::unshare_file,
            commands::quit_session,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
