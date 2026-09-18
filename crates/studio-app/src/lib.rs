//! Everything the UI asks of the backend, as plain blocking methods on [`StudioApp`].
//!
//! The desktop app wraps these in Tauri commands; `studio-server` serves them over
//! stdio to the VS Code extension. Session output (sample frames and events) goes to
//! the [`SessionSink`](studio_core::SessionSink) passed to [`StudioApp::connect`].

pub mod elf;
pub mod session;

pub use elf::{startup_elf_path, OpenedElf};
pub use session::{
    list_probes, list_serial_ports, search_chips, Carrier, ConnectRequest, ProbeOpener,
    TaskSnapshot, ValueRead, WatchRequest, WatchResult,
};
pub use studio_core::{SessionEvent, SessionSink};

use elf::LoadedElf;
use session::SessionState;

/// The loaded ELF and the running session, shared by every request.
pub struct StudioApp {
    elf: LoadedElf,
    session: SessionState,
    probe_opener: ProbeOpener,
    /// Read the defmt log and serve tuning over the firmware's RTT channels
    rtt: bool,
}

impl Default for StudioApp {
    fn default() -> Self {
        Self::new()
    }
}

impl StudioApp {
    /// Connects to real probes.
    pub fn new() -> Self {
        Self::with_probe_opener(session::real_probe())
    }

    /// Opens the probe carrier with `opener` instead, e.g. a mock target for tests.
    pub fn with_probe_opener(opener: ProbeOpener) -> Self {
        Self {
            elf: LoadedElf::default(),
            session: SessionState::default(),
            probe_opener: opener,
            rtt: true,
        }
    }

    /// How this app opens the probe carrier
    pub fn probe_opener(&self) -> ProbeOpener {
        self.probe_opener.clone()
    }

    /// Leave the firmware's RTT channels alone: no log, no tuning over RTT. Reading
    /// the log moves the channel's read offset, which is a write to target memory.
    pub fn without_rtt(mut self) -> Self {
        self.rtt = false;
        self
    }
}
