mod commands;

use std::sync::Arc;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    studio_carriers::probe::init_hid_on_main_thread();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage::<commands::App>(Arc::new(studio_app::StudioApp::new()))
        .invoke_handler(tauri::generate_handler![
            commands::open_elf,
            commands::symbol_children,
            commands::startup_elf_path,
            commands::list_probes,
            commands::search_chips,
            commands::session_connect,
            commands::session_disconnect,
            commands::session_set_watches,
            commands::session_set_rate,
            commands::session_request,
            commands::session_discard,
            commands::session_save,
            commands::list_serial_ports,
            commands::watchable_leaves,
            commands::session_task_states,
            commands::session_read_values,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
