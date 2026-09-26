mod commands;
mod core;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            commands::check_credentials,
            commands::os_platform,
            commands::get_file_info,
            commands::scan_paths,
            commands::decrypt_paths,
            commands::library_status,
            commands::flush_library_links,
        ])
        .run(tauri::generate_context!())
        .expect("error while running QM Unlock");
}
