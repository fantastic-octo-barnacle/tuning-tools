//! Probe session commands. The session thread (studio-core) owns the probe;
//! these commands only start, stop and steer it. Samples reach the webview as
//! binary frames on one channel, status, stats and log lines as JSON on another.

use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use studio_carriers::probe::{self, ProbeConfig, ProbeInfo, ProbeLink};
use studio_carriers::Link;
use studio_core::plan::scalar_len;
use studio_core::session::SessionOptions;
use studio_core::{ReadItem, Session, SessionCommand, SessionEvent, SessionSink};
use studio_dwarf::tree::{self, NodeKind};
use studio_dwarf::{ElfInfo, NodeRef, SymbolNode};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::State;

use crate::elf::LoadedElf;

/// Scalars collected by [`watchable_leaves`] before it stops.
const MAX_LEAVES: usize = 256;

#[derive(Default)]
pub struct SessionState {
    session: Mutex<Option<Session>>,
    /// Last watched set, re-sent when a new session connects
    watches: Mutex<Vec<ReadItem>>,
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectRequest {
    probe: Option<String>,
    chip: String,
    speed_khz: Option<u32>,
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
    let elf = elf.current()?;
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
        rtt_address: elf.find_symbol("_SEGGER_RTT").map(|s| s.address),
    };
    let session = Session::spawn(
        move || ProbeLink::open(&config).map(|link| Box::new(link) as Box<dyn Link>),
        SessionOptions {
            rate_hz: request.rate_hz,
            elf: Some(elf_bytes),
        },
        Arc::new(ChannelSink { data, events }),
    );
    let watches = state
        .watches
        .lock()
        .expect("session state poisoned")
        .clone();
    if !watches.is_empty() {
        session.send(SessionCommand::SetWatches(watches));
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

#[derive(Deserialize)]
pub struct WatchRequest {
    id: u32,
    node: NodeRef,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchResult {
    id: u32,
    /// Why the value cannot be sampled; `None` when it is
    error: Option<String>,
}

/// Resolve watches against the loaded ELF and hand the readable ones to the session.
#[tauri::command]
pub fn session_set_watches(
    watches: Vec<WatchRequest>,
    elf: State<'_, LoadedElf>,
    state: State<'_, SessionState>,
) -> Result<Vec<WatchResult>, String> {
    let elf = elf.current()?;
    let mut items = Vec::new();
    let results = watches
        .into_iter()
        .map(|w| match resolve(&elf, &w.node) {
            Ok(mut item) => {
                item.id = w.id;
                items.push(item);
                WatchResult {
                    id: w.id,
                    error: None,
                }
            }
            Err(error) => WatchResult {
                id: w.id,
                error: Some(error),
            },
        })
        .collect();
    if let Some(session) = state
        .session
        .lock()
        .expect("session state poisoned")
        .as_ref()
    {
        session.send(SessionCommand::SetWatches(items.clone()));
    }
    *state.watches.lock().expect("session state poisoned") = items;
    Ok(results)
}

#[tauri::command]
pub fn session_set_rate(hz: f64, state: State<'_, SessionState>) {
    if let Some(session) = state
        .session
        .lock()
        .expect("session state poisoned")
        .as_ref()
    {
        session.send(SessionCommand::SetRate(hz));
    }
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
