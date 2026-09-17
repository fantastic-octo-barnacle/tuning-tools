//! Debug probe carrier over probe-rs: SWD memory access on core 0 and RTT.

use std::time::{Duration, Instant};

use probe_rs::config::Registry;
use probe_rs::probe::list::Lister;
use probe_rs::probe::DebugProbeSelector;
use probe_rs::rtt::{Rtt, ScanRegion};
use probe_rs::{CoreStatus, MemoryInterface, Permissions, Session};
use serde::{Deserialize, Serialize};

use crate::{ByteStream, CarrierError, CoreState, Link, MemoryAccess, Result, StreamState};

/// How often to look for the RTT control block before the firmware sets it up.
const RTT_RETRY: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeInfo {
    /// Pass back to [`ProbeLink::open`]; `VID:PID` or `VID:PID:SERIAL`
    pub selector: String,
    pub name: String,
    pub serial: Option<String>,
}

pub fn list_probes() -> Vec<ProbeInfo> {
    Lister::new()
        .list_all()
        .iter()
        .map(|p| ProbeInfo {
            selector: DebugProbeSelector::from(p).to_string(),
            name: p.identifier.clone(),
            serial: p.serial_number.clone(),
        })
        .collect()
}

/// Chip names probe-rs knows, containing `query` (case-insensitive), at most `limit`.
pub fn search_chips(query: &str, limit: usize) -> Vec<String> {
    let needle = query.to_ascii_lowercase();
    let registry = Registry::from_builtin_families();
    let mut names: Vec<String> = registry
        .families()
        .iter()
        .flat_map(|f| f.variants.iter().map(|c| c.name.clone()))
        .filter(|name| name.to_ascii_lowercase().contains(&needle))
        .collect();
    names.sort();
    names.truncate(limit);
    names
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeConfig {
    /// From [`ProbeInfo::selector`]; `None` takes the only probe attached
    pub selector: Option<String>,
    /// probe-rs chip name, e.g. `STM32H723VG`
    pub chip: String,
    pub speed_khz: Option<u32>,
    /// Address of `_SEGGER_RTT` from the ELF; `None` means no log stream
    pub rtt_address: Option<u64>,
}

pub struct ProbeLink {
    session: Session,
    rtt: RttState,
}

impl ProbeLink {
    /// Attach without halting or resetting: the firmware keeps running.
    pub fn open(config: &ProbeConfig) -> Result<Self> {
        let lister = Lister::new();
        let mut probe = match &config.selector {
            Some(selector) => {
                let parsed: DebugProbeSelector = selector
                    .parse()
                    .map_err(|e| CarrierError::Probe(format!("{e}")))?;
                lister.open(parsed).map_err(|e| match e {
                    probe_rs::probe::DebugProbeError::ProbeCouldNotBeCreated(_) => {
                        CarrierError::ProbeNotFound(selector.clone())
                    }
                    other => CarrierError::Probe(other.to_string()),
                })?
            }
            None => {
                let probes = lister.list_all();
                match probes.as_slice() {
                    [only] => only
                        .open()
                        .map_err(|e| CarrierError::Probe(e.to_string()))?,
                    [] => return Err(CarrierError::ProbeNotFound("any probe".into())),
                    _ => {
                        return Err(CarrierError::Probe(format!(
                            "{} probes attached; pick one",
                            probes.len()
                        )))
                    }
                }
            }
        };
        if let Some(khz) = config.speed_khz {
            probe
                .set_speed(khz)
                .map_err(|e| CarrierError::Probe(e.to_string()))?;
        }
        let session = probe
            .attach(config.chip.as_str(), Permissions::default())
            .map_err(|e| CarrierError::Attach(e.to_string()))?;
        Ok(Self {
            session,
            rtt: match config.rtt_address {
                Some(address) => RttState::Searching {
                    address,
                    last_try: None,
                },
                None => RttState::Absent,
            },
        })
    }
}

enum RttState {
    Absent,
    Searching {
        address: u64,
        last_try: Option<Instant>,
    },
    Attached {
        rtt: Box<Rtt>,
        channel: String,
    },
}

impl Link for ProbeLink {
    fn memory(&mut self) -> &mut dyn MemoryAccess {
        self
    }

    fn log(&mut self) -> &mut dyn ByteStream {
        self
    }

    fn core_state(&mut self) -> Result<CoreState> {
        let mut core = self
            .session
            .core(0)
            .map_err(|e| CarrierError::Other(e.to_string()))?;
        let status = core
            .status()
            .map_err(|e| CarrierError::Other(e.to_string()))?;
        Ok(match status {
            CoreStatus::Running => CoreState::Running,
            CoreStatus::Halted(_) => CoreState::Halted,
            CoreStatus::Sleeping => CoreState::Sleeping,
            CoreStatus::LockedUp => CoreState::LockedUp,
            CoreStatus::Unknown => CoreState::Unknown,
        })
    }
}

impl MemoryAccess for ProbeLink {
    fn read(&mut self, address: u64, buf: &mut [u8]) -> Result<()> {
        let len = buf.len();
        let err = |reason: String| CarrierError::Read {
            address,
            len,
            reason,
        };
        let mut core = self.session.core(0).map_err(|e| err(e.to_string()))?;
        core.read(address, buf).map_err(|e| err(e.to_string()))
    }

    fn write(&mut self, address: u64, data: &[u8]) -> Result<()> {
        let err = |reason: String| CarrierError::Write {
            address,
            len: data.len(),
            reason,
        };
        let mut core = self.session.core(0).map_err(|e| err(e.to_string()))?;
        core.write(address, data).map_err(|e| err(e.to_string()))
    }
}

impl ByteStream for ProbeLink {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let mut core = self
            .session
            .core(0)
            .map_err(|e| CarrierError::Other(e.to_string()))?;
        match &mut self.rtt {
            RttState::Absent => Ok(0),
            RttState::Searching { address, last_try } => {
                if last_try.is_some_and(|t| t.elapsed() < RTT_RETRY) {
                    return Ok(0);
                }
                *last_try = Some(Instant::now());
                // Before the firmware runs `rtt_init` the block has no valid ID yet
                let Ok(mut rtt) = Rtt::attach_region(&mut core, &ScanRegion::Exact(*address))
                else {
                    return Ok(0);
                };
                let Some(up) = rtt.up_channel(0) else {
                    return Ok(0);
                };
                let channel = up.name().unwrap_or("up 0").to_string();
                tracing::info!(%channel, address = *address, "RTT attached");
                self.rtt = RttState::Attached {
                    rtt: Box::new(rtt),
                    channel,
                };
                Ok(0)
            }
            RttState::Attached { rtt, .. } => {
                let up = rtt
                    .up_channel(0)
                    .ok_or_else(|| CarrierError::Other("RTT up channel 0 vanished".into()))?;
                up.read(&mut core, buf)
                    .map_err(|e| CarrierError::Other(format!("RTT read failed: {e}")))
            }
        }
    }

    fn state(&self) -> StreamState {
        match &self.rtt {
            RttState::Absent => StreamState::Absent,
            RttState::Searching { .. } => StreamState::Searching,
            RttState::Attached { channel, .. } => StreamState::Attached {
                channel: channel.clone(),
            },
        }
    }
}
