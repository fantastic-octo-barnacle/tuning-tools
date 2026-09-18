//! Session commands. The session thread (studio-core) owns the probe or the
//! serial port; these commands only start, stop and steer it. Samples reach the webview as
//! binary frames on one channel, status, stats and log lines as JSON on another.

use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use studio_carriers::probe::{self, ProbeConfig, ProbeInfo, ProbeLink};
use studio_carriers::serial::{self, PortInfo, SerialStream};
use studio_carriers::{ByteStream, Link};
use studio_core::catalog::{Catalog, TableLayout};
use studio_core::link::{spawn_link, LinkOptions};
use studio_core::plan::scalar_len;
use studio_core::session::SessionOptions;
use studio_core::{ReadItem, Session, SessionCommand, SessionEvent, SessionSink};
use studio_dwarf::tasks::{self, TaskState};
use studio_dwarf::tree::{self, NodeKind};
use studio_dwarf::{ElfInfo, NodeRef, SymbolNode};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::State;

use crate::elf::{LoadedElf, Tuning};

/// Scalars collected by [`watchable_leaves`] before it stops.
const MAX_LEAVES: usize = 256;
/// Longest a tuning write waits for the session thread
const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct SessionState {
    session: Mutex<Option<Session>>,
    /// Last watched set, re-sent when a new session connects
    watches: Mutex<Vec<ReadItem>>,
    /// Tuning values watched by id, as `(watch id, value id)`, for a framed link
    cell_watches: Mutex<Vec<(u32, u32)>>,
}

impl SessionState {
    /// Hand a newly opened ELF's tuning table to a running session.
    pub fn set_tuning(&self, tuning: Option<Tuning>) {
        if let Some(session) = self
            .session
            .lock()
            .expect("session state poisoned")
            .as_ref()
        {
            session.send(catalog_command(tuning));
        }
    }
}

fn catalog_command(tuning: Option<Tuning>) -> SessionCommand {
    SessionCommand::SetCatalog(tuning.map(|t| Box::new((*t).clone())))
}

struct ChannelSink {
    data: Channel<InvokeResponseBody>,
    events: Channel<SessionEvent>,
}

impl SessionSink for ChannelSink {
    fn frame(&self, bytes: Vec<u8>) -> bool {
        self.data.send(InvokeResponseBody::Raw(bytes)).is_ok()
    }

    fn event(&self, event: SessionEvent) {
        let _ = self.events.send(event);
    }
}

impl SessionState {
    fn send(&self, command: SessionCommand) -> bool {
        self.session
            .lock()
            .expect("session state poisoned")
            .as_ref()
            .is_some_and(|s| s.send(command))
    }
}

#[tauri::command]
pub async fn list_probes() -> Result<Vec<ProbeInfo>, String> {
    tauri::async_runtime::spawn_blocking(probe::list_probes)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn search_chips(query: String) -> Result<Vec<String>, String> {
    static ALL: OnceLock<Vec<String>> = OnceLock::new();
    tauri::async_runtime::spawn_blocking(move || {
        let all = ALL.get_or_init(|| probe::search_chips("", usize::MAX));
        let needle = query.to_ascii_lowercase();
        all.iter()
            .filter(|name| name.to_ascii_lowercase().contains(&needle))
            .take(50)
            .cloned()
            .collect()
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn list_serial_ports() -> Result<Vec<PortInfo>, String> {
    tauri::async_runtime::spawn_blocking(serial::list_ports)
        .await
        .map_err(|e| e.to_string())
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum Carrier {
    /// A debug probe on SWD: needs the ELF and the chip
    Probe,
    /// The firmware's framed link on a serial port: needs neither
    Serial,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectRequest {
    carrier: Carrier,
    probe: Option<String>,
    chip: String,
    speed_khz: Option<u32>,
    port: Option<String>,
    rate_hz: f64,
}

#[tauri::command]
pub async fn session_connect(
    request: ConnectRequest,
    data: Channel<InvokeResponseBody>,
    events: Channel<SessionEvent>,
    elf: State<'_, LoadedElf>,
    state: State<'_, SessionState>,
) -> Result<(), String> {
    if request.carrier == Carrier::Serial {
        return connect_serial(request, ChannelSink { data, events }, &state).await;
    }
    let elf_state = elf;
    let elf = elf_state.current()?;
    if request.chip.trim().is_empty() {
        return Err("choose the target chip first".into());
    }
    let old = state.session.lock().expect("session state poisoned").take();
    let path = elf.path.clone();
    // Stopping joins the old thread and the probe detaches; reading the ELF is disk I/O
    let elf_bytes = tauri::async_runtime::spawn_blocking(move || {
        drop(old);
        std::fs::read(&path)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("could not read {}: {e}", elf.path))?;

    let config = ProbeConfig {
        selector: request.probe.filter(|s| !s.is_empty()),
        chip: request.chip.trim().to_string(),
        speed_khz: request.speed_khz,
    };
    let rtt_address = elf.find_symbol("_SEGGER_RTT").map(|s| s.address);
    let tuning = elf_state.tuning()?;
    let session = Session::spawn(
        move || ProbeLink::open(&config).map(|link| Box::new(link) as Box<dyn Link>),
        SessionOptions {
            rate_hz: request.rate_hz,
            elf: Some(elf_bytes),
            rtt_address,
        },
        Arc::new(ChannelSink { data, events }),
    );
    let watches = state
        .watches
        .lock()
        .expect("session state poisoned")
        .clone();
    if tuning.is_some() {
        session.send(catalog_command(tuning));
    }
    if !watches.is_empty() {
        session.send(SessionCommand::SetWatches(watches));
    }
    *state.session.lock().expect("session state poisoned") = Some(session);
    Ok(())
}

async fn connect_serial(
    request: ConnectRequest,
    sink: ChannelSink,
    state: &SessionState,
) -> Result<(), String> {
    let port = request
        .port
        .filter(|p| !p.is_empty())
        .ok_or("choose the serial port first")?;
    let old = state.session.lock().expect("session state poisoned").take();
    tauri::async_runtime::spawn_blocking(move || drop(old))
        .await
        .map_err(|e| e.to_string())?;
    let session = spawn_link(
        move || SerialStream::open(&port, 115_200).map(|s| Box::new(s) as Box<dyn ByteStream>),
        LinkOptions {
            rate_hz: request.rate_hz,
        },
        Arc::new(sink),
    );
    let watches = state
        .cell_watches
        .lock()
        .expect("session state poisoned")
        .clone();
    if !watches.is_empty() {
        session.send(SessionCommand::SetCellWatches(watches));
    }
    *state.session.lock().expect("session state poisoned") = Some(session);
    Ok(())
}

#[tauri::command]
pub async fn session_disconnect(state: State<'_, SessionState>) -> Result<(), String> {
    let old = state.session.lock().expect("session state poisoned").take();
    tauri::async_runtime::spawn_blocking(move || drop(old))
        .await
        .map_err(|e| e.to_string())
}

/// A value to sample: a symbol path, or a tuning table value by id
#[derive(Deserialize)]
pub struct WatchRequest {
    id: u32,
    node: Option<NodeRef>,
    cell: Option<u32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchResult {
    id: u32,
    /// Why the value cannot be sampled; `None` when it is
    error: Option<String>,
}

/// Resolve watches and hand the sampleable ones to the session. Symbols
/// resolve against the loaded ELF for a probe; tuning values also go by id to
/// a framed link, which needs no ELF.
#[tauri::command]
pub fn session_set_watches(
    watches: Vec<WatchRequest>,
    elf: State<'_, LoadedElf>,
    state: State<'_, SessionState>,
) -> Result<Vec<WatchResult>, String> {
    let cells: Vec<(u32, u32)> = watches
        .iter()
        .filter_map(|w| w.cell.map(|cell| (w.id, cell)))
        .collect();
    state.send(SessionCommand::SetCellWatches(cells.clone()));
    *state.cell_watches.lock().expect("session state poisoned") = cells;

    let loaded = elf
        .current()
        .ok()
        .map(|info| (info, elf.tuning().ok().flatten()));
    let mut items = Vec::new();
    let results = watches
        .into_iter()
        .map(|w| {
            let resolved = match &loaded {
                Some((info, tuning)) => resolve_watch(info, tuning.as_deref(), &w).map(Some),
                None if w.cell.is_some() => Ok(None),
                None => Err("open the ELF to watch symbols".to_string()),
            };
            let error = match resolved {
                Ok(Some(mut item)) => {
                    item.id = w.id;
                    items.push(item);
                    None
                }
                Ok(None) => None,
                Err(error) => Some(error),
            };
            WatchResult { id: w.id, error }
        })
        .collect();
    state.send(SessionCommand::SetWatches(items.clone()));
    *state.watches.lock().expect("session state poisoned") = items;
    Ok(results)
}

/// Ask the firmware to go back to every tunable's built-in default.
#[tauri::command]
pub async fn session_discard(state: State<'_, SessionState>) -> Result<(), String> {
    let (reply, rx) = mpsc::sync_channel(1);
    if !state.send(SessionCommand::Discard { reply }) {
        return Err("connect to the target first".into());
    }
    wait_reply(rx).await
}

/// Ask the firmware to keep every current value across a power cycle.
#[tauri::command]
pub async fn session_save(state: State<'_, SessionState>) -> Result<(), String> {
    let (reply, rx) = mpsc::sync_channel(1);
    if !state.send(SessionCommand::Save { reply }) {
        return Err("connect to the target first".into());
    }
    wait_reply_for(rx, studio_core::link::SAVE_TIMEOUT + REQUEST_TIMEOUT).await
}

async fn wait_reply(rx: mpsc::Receiver<Result<(), String>>) -> Result<(), String> {
    wait_reply_for(rx, REQUEST_TIMEOUT).await
}

async fn wait_reply_for(
    rx: mpsc::Receiver<Result<(), String>>,
    timeout: Duration,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        rx.recv_timeout(timeout)
            .unwrap_or_else(|_| Err("the target did not take the write in time".into()))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn session_set_rate(hz: f64, state: State<'_, SessionState>) {
    state.send(SessionCommand::SetRate(hz));
}

fn resolve_watch(
    elf: &ElfInfo,
    tuning: Option<&(TableLayout, Catalog)>,
    watch: &WatchRequest,
) -> Result<ReadItem, String> {
    match (&watch.node, watch.cell) {
        (Some(node), _) => resolve(elf, node),
        (None, Some(cell)) => tuning
            .and_then(|(_, catalog)| catalog.entry(cell))
            .map(|entry| entry.applied_item(0))
            .ok_or_else(|| "this ELF's tuning table has no such value".into()),
        (None, None) => Err("nothing to watch".into()),
    }
}

/// Ask the firmware to run tuning value `id` at `value`.
#[tauri::command]
pub async fn session_request(
    id: u32,
    value: f64,
    state: State<'_, SessionState>,
) -> Result<(), String> {
    let (reply, rx) = mpsc::sync_channel(1);
    if !state.send(SessionCommand::Request { id, value, reply }) {
        return Err("connect to the target first".into());
    }
    wait_reply(rx).await
}

fn resolve(elf: &ElfInfo, node: &NodeRef) -> Result<ReadItem, String> {
    let n = tree::node(elf, node).map_err(|e| e.to_string())?;
    if !n.readable {
        return Err(n.status.unwrap_or_else(|| "not readable".into()));
    }
    let scalar = n
        .scalar
        .filter(|s| scalar_len(*s).is_some())
        .ok_or_else(|| format!("{} is not a single number", n.type_name))?;
    Ok(ReadItem {
        id: 0,
        address: n.address,
        scalar,
        bit_offset: n.bit_offset,
        bit_size: n.bit_size,
    })
}

/// Numeric leaves under `node` (the node itself when it is one), depth first.
#[tauri::command]
pub fn watchable_leaves(
    node: NodeRef,
    elf: State<'_, LoadedElf>,
) -> Result<Vec<SymbolNode>, String> {
    let elf = elf.current()?;
    let start = tree::node(&elf, &node).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    collect_leaves(&elf, start, &mut out)?;
    Ok(out)
}

fn collect_leaves(
    elf: &ElfInfo,
    node: SymbolNode,
    out: &mut Vec<SymbolNode>,
) -> Result<(), String> {
    if out.len() >= MAX_LEAVES || !node.readable {
        return Ok(());
    }
    match node.kind {
        NodeKind::Scalar | NodeKind::Enum => {
            if node.scalar.is_some_and(|s| scalar_len(s).is_some()) {
                out.push(node);
            }
        }
        // A tagged enum's payload depends on the live tag; only the tag is safe to sample
        NodeKind::TaggedEnum => {
            let children = tree::children(elf, &node.node, None).map_err(|e| e.to_string())?;
            if let Some(tag) = children
                .nodes
                .into_iter()
                .find(|c| c.node.steps.last() == Some(&studio_dwarf::Step::Discriminant))
            {
                collect_leaves(elf, tag, out)?;
            }
        }
        NodeKind::Struct | NodeKind::Array => {
            let children = tree::children(elf, &node.node, None).map_err(|e| e.to_string())?;
            for child in children.nodes {
                collect_leaves(elf, child, out)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatus {
    /// The task slot's node path, as `Task.root.path`
    path: String,
    state: Option<TaskState>,
    /// Why the state could not be read
    error: Option<String>,
}

/// Read every embassy task's state in one pass over target memory.
#[tauri::command]
pub async fn session_task_states(
    elf: State<'_, LoadedElf>,
    state: State<'_, SessionState>,
) -> Result<Vec<TaskStatus>, String> {
    let elf = elf.current()?;
    let probes: Vec<_> = tasks::tasks(&elf)
        .into_iter()
        .map(|t| {
            let probe = tasks::probe(&elf, &t.root.node.node).map_err(|e| e.to_string());
            (t.root.node.path, probe)
        })
        .collect();
    let regions: Vec<(u64, usize)> = probes
        .iter()
        .filter_map(|(_, p)| p.as_ref().ok())
        .flat_map(|p| p.regions())
        .collect();
    let (reply, rx) = mpsc::sync_channel(1);
    if !state.send(SessionCommand::Read { regions, reply }) {
        return Err("connect a debug probe first".into());
    }
    let bytes = tauri::async_runtime::spawn_blocking(move || {
        rx.recv_timeout(REQUEST_TIMEOUT)
            .unwrap_or_else(|_| Err("the target did not answer in time".into()))
    })
    .await
    .map_err(|e| e.to_string())??;

    let mut bytes = bytes.into_iter();
    Ok(probes
        .into_iter()
        .map(|(path, probe)| match probe {
            Ok(probe) => {
                let mine: Vec<Vec<u8>> = bytes.by_ref().take(probe.regions().len()).collect();
                let state = probe.decode(&mine);
                TaskStatus {
                    path,
                    error: state.is_none().then(|| "short read".to_string()),
                    state,
                }
            }
            Err(error) => TaskStatus {
                path,
                state: None,
                error: Some(error),
            },
        })
        .collect())
}
