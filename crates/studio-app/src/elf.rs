//! ELF loading and symbol browsing. The parsed `ElfInfo` is kept so tree
//! expansion does not re-read the file.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;
use studio_core::catalog::{Catalog, ElfImage, TableLayout};
use studio_dwarf::task_stats::StatsLayout;
use studio_dwarf::tasks::{self, Task, TaskProbe};
use studio_dwarf::tree::{self, Children, ElfSummary, RootNode};
use studio_dwarf::{ElfInfo, ElfParser, NodeRef};

use crate::StudioApp;

pub type Tuning = Arc<(TableLayout, Catalog)>;
/// How to read each task's state, as `(task path, slot address, probe)`,
/// worked out once per ELF
pub type TaskProbes = Arc<Vec<(String, u64, Result<TaskProbe, String>)>>;

/// The firmware's per-task counters: `None` when it has none, or why they cannot be read
pub type TaskStats = Option<Result<Arc<StatsLayout>, String>>;

#[derive(Clone)]
struct Loaded {
    elf: Arc<ElfInfo>,
    tuning: Option<Tuning>,
    probes: TaskProbes,
    stats: TaskStats,
}

#[derive(Default)]
pub struct LoadedElf(Mutex<Option<Loaded>>);

impl LoadedElf {
    pub fn current(&self) -> Result<Arc<ElfInfo>, String> {
        self.loaded().map(|l| l.elf)
    }

    /// The ELF's tuning table, when it declares one
    pub fn tuning(&self) -> Result<Option<Tuning>, String> {
        self.loaded().map(|l| l.tuning)
    }

    pub fn task_probes(&self) -> Result<(TaskProbes, TaskStats), String> {
        self.loaded().map(|l| (l.probes, l.stats))
    }

    fn loaded(&self) -> Result<Loaded, String> {
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
    pub summary: ElfSummary,
    pub roots: Vec<RootNode>,
    /// embassy task slots, browsable like roots
    pub tasks: Vec<Task>,
    pub parse_ms: u64,
    pub catalog: Option<Catalog>,
    /// Why a declared tuning table could not be read
    pub catalog_error: Option<String>,
}

impl StudioApp {
    /// Parse the ELF and make it the current one. Blocks on disk I/O and DWARF parsing.
    pub fn open_elf(&self, path: PathBuf) -> Result<OpenedElf, String> {
        let started = Instant::now();
        let elf = ElfParser::parse(&path).map_err(|e| e.to_string())?;
        let tuning = read_catalog(&elf);
        let elf = Arc::new(elf);
        let (tuning, catalog_error) = match tuning {
            Ok(tuning) => (tuning.map(Arc::new), None),
            Err(e) => (None, Some(e)),
        };
        let tasks = tasks::tasks(&elf);
        let probes = tasks
            .iter()
            .map(|t| {
                let probe = tasks::probe(&elf, &t.root.node.node).map_err(|e| e.to_string());
                (t.root.node.path.clone(), t.root.node.address, probe)
            })
            .collect();
        let stats = StatsLayout::find(&elf)
            .map_err(|e| e.to_string())
            .transpose()
            .map(|s| s.map(Arc::new));
        let opened = OpenedElf {
            summary: tree::summary(&elf),
            roots: tree::roots(&elf),
            tasks,
            parse_ms: started.elapsed().as_millis() as u64,
            catalog: tuning.as_ref().map(|t| t.1.clone()),
            catalog_error,
        };
        self.session.set_tuning(tuning.clone());
        *self.elf.0.lock().expect("elf state poisoned") = Some(Loaded {
            elf,
            tuning,
            probes: Arc::new(probes),
            stats,
        });
        Ok(opened)
    }

    pub fn symbol_children(
        &self,
        node: &NodeRef,
        limit: Option<usize>,
    ) -> Result<Children, String> {
        let elf = self.elf.current()?;
        tree::children(&elf, node, limit).map_err(|e| e.to_string())
    }
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

/// ELF to open at startup, from `TUNING_TOOLS_ELF` (development convenience).
pub fn startup_elf_path() -> Option<String> {
    std::env::var("TUNING_TOOLS_ELF")
        .ok()
        .filter(|p| !p.is_empty())
}
