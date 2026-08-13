// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod jobs;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(jobs::Jobs::default())
        .invoke_handler(tauri::generate_handler![
            commands::transcribe_file,
            commands::cancel_job
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Transcriba app");
}
