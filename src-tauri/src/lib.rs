mod elf;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(elf::LoadedElf::default())
        .invoke_handler(tauri::generate_handler![
            elf::open_elf,
            elf::symbol_children,
            elf::startup_elf_path
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
