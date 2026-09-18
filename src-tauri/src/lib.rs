mod elf;
mod session;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    studio_carriers::probe::init_hid_on_main_thread();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(elf::LoadedElf::default())
        .manage(session::SessionState::default())
        .invoke_handler(tauri::generate_handler![
            elf::open_elf,
            elf::symbol_children,
            elf::startup_elf_path,
            session::list_probes,
            session::search_chips,
            session::session_connect,
            session::session_disconnect,
            session::session_set_watches,
            session::session_set_rate,
            session::session_request,
            session::session_discard,
            session::session_save,
            session::list_serial_ports,
            session::watchable_leaves
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
