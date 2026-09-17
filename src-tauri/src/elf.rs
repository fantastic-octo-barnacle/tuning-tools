//! ELF loading and symbol browsing commands. Parsing runs on a blocking thread;
//! the parsed `ElfInfo` is kept so tree expansion does not re-read the file.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;
use studio_dwarf::tree::{self, Children, ElfSummary, RootNode};
use studio_dwarf::{ElfInfo, ElfParser, NodeRef};
use tauri::State;

#[derive(Default)]
pub struct LoadedElf(Mutex<Option<Arc<ElfInfo>>>);

impl LoadedElf {
    pub fn current(&self) -> Result<Arc<ElfInfo>, String> {
        self.0
            .lock()
            .expect("elf state poisoned")
            .clone()
            .ok_or_else(|| "no ELF loaded".to_string())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenedElf {
    summary: ElfSummary,
    roots: Vec<RootNode>,
    parse_ms: u64,
}

#[tauri::command]
pub async fn open_elf(path: PathBuf, state: State<'_, LoadedElf>) -> Result<OpenedElf, String> {
    let started = Instant::now();
    let elf = tauri::async_runtime::spawn_blocking(move || ElfParser::parse(&path))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let elf = Arc::new(elf);
    let opened = OpenedElf {
        summary: tree::summary(&elf),
        roots: tree::roots(&elf),
        parse_ms: started.elapsed().as_millis() as u64,
    };
    *state.0.lock().expect("elf state poisoned") = Some(elf);
    Ok(opened)
}

#[tauri::command]
pub fn symbol_children(
    node: NodeRef,
    limit: Option<usize>,
    state: State<'_, LoadedElf>,
) -> Result<Children, String> {
    let elf = state.current()?;
    tree::children(&elf, &node, limit).map_err(|e| e.to_string())
}

/// ELF to open at startup, from `TUNING_TOOLS_ELF` (development convenience).
#[tauri::command]
pub fn startup_elf_path() -> Option<String> {
    std::env::var("TUNING_TOOLS_ELF")
        .ok()
        .filter(|p| !p.is_empty())
}
