//! ELF loading and symbol browsing commands. Parsing runs on a blocking thread;
//! the parsed `ElfInfo` is kept so tree expansion does not re-read the file.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;
use studio_core::catalog::{Catalog, ElfImage, TableLayout};
use studio_dwarf::tasks::{self, Task};
use studio_dwarf::tree::{self, Children, ElfSummary, RootNode};
use studio_dwarf::{ElfInfo, ElfParser, NodeRef};
use tauri::State;

use crate::session::SessionState;

pub type Tuning = Arc<(TableLayout, Catalog)>;

#[derive(Default)]
pub struct LoadedElf(Mutex<Option<(Arc<ElfInfo>, Option<Tuning>)>>);

impl LoadedElf {
    pub fn current(&self) -> Result<Arc<ElfInfo>, String> {
        self.loaded().map(|(elf, _)| elf)
    }

    /// The ELF's tuning table, when it declares one
    pub fn tuning(&self) -> Result<Option<Tuning>, String> {
        self.loaded().map(|(_, tuning)| tuning)
    }

    fn loaded(&self) -> Result<(Arc<ElfInfo>, Option<Tuning>), String> {
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
    /// embassy task slots, browsable like roots
    tasks: Vec<Task>,
    parse_ms: u64,
    catalog: Option<Catalog>,
    /// Why a declared tuning table could not be read
    catalog_error: Option<String>,
}

#[tauri::command]
pub async fn open_elf(
    path: PathBuf,
    state: State<'_, LoadedElf>,
    session: State<'_, SessionState>,
) -> Result<OpenedElf, String> {
    let started = Instant::now();
    let (elf, tuning) = tauri::async_runtime::spawn_blocking(move || {
        let elf = ElfParser::parse(&path).map_err(|e| e.to_string())?;
        let tuning = read_catalog(&elf);
        Ok::<_, String>((elf, tuning))
    })
    .await
    .map_err(|e| e.to_string())??;
    let elf = Arc::new(elf);
    let (tuning, catalog_error) = match tuning {
        Ok(tuning) => (tuning.map(Arc::new), None),
        Err(e) => (None, Some(e)),
    };
    let opened = OpenedElf {
        summary: tree::summary(&elf),
        roots: tree::roots(&elf),
        tasks: tasks::tasks(&elf),
        parse_ms: started.elapsed().as_millis() as u64,
        catalog: tuning.as_ref().map(|t| t.1.clone()),
        catalog_error,
    };
    session.set_tuning(tuning.clone());
    *state.0.lock().expect("elf state poisoned") = Some((elf, tuning));
    Ok(opened)
}

/// Decode the tuning table from the file alone: its cells are initialised
/// statics, so the ELF holds the same bytes the target starts with.
fn read_catalog(elf: &ElfInfo) -> Result<Option<(TableLayout, Catalog)>, String> {
    let Some(layout) = TableLayout::find(elf).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let bytes =
        std::fs::read(&elf.path).map_err(|e| format!("could not read {}: {e}", elf.path))?;
    let mut image = ElfImage::parse(&bytes)?;
    let catalog = layout.read(&mut image).map_err(|e| e.to_string())?;
    Ok(Some((layout, catalog)))
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
