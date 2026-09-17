//! Carriers: how the studio reaches a target.
//!
//! A carrier gives random access to target memory ([`MemoryAccess`]) and, when
//! the firmware has one, a byte stream from the target ([`ByteStream`], the
//! defmt log on RTT up 0). Every carrier is owned by one hardware thread; the
//! traits take `&mut self` and nothing here is shared.

pub mod mock;
pub mod probe;

use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum CarrierError {
    #[error("no debug probe matches `{0}`")]
    ProbeNotFound(String),
    #[error("could not open the probe: {0}")]
    Probe(String),
    #[error("could not attach to the target: {0}")]
    Attach(String),
    #[error("read of {len} bytes at {address:#010x} failed: {reason}")]
    Read {
        address: u64,
        len: usize,
        reason: String,
    },
    #[error("write of {len} bytes at {address:#010x} failed: {reason}")]
    Write {
        address: u64,
        len: usize,
        reason: String,
    },
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, CarrierError>;

/// Random access to target memory. Reads must not halt the core.
pub trait MemoryAccess {
    fn read(&mut self, address: u64, buf: &mut [u8]) -> Result<()>;
    fn write(&mut self, address: u64, data: &[u8]) -> Result<()>;
}

/// Bytes the target sends on its own. `read` never blocks.
pub trait ByteStream {
    /// Copy pending bytes into `buf`; `Ok(0)` when there are none yet.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize>;
    fn state(&self) -> StreamState;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum StreamState {
    /// The firmware has no stream (no RTT control block in the ELF)
    Absent,
    /// Looking for the control block; firmware may not have initialised it yet
    Searching,
    Attached {
        channel: String,
    },
}

/// Run state of the core, polled at a low rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CoreState {
    Running,
    Halted,
    Sleeping,
    LockedUp,
    Unknown,
}

/// A connected target: memory plus the log stream.
pub trait Link: Send {
    fn memory(&mut self) -> &mut dyn MemoryAccess;
    fn log(&mut self) -> &mut dyn ByteStream;
    fn core_state(&mut self) -> Result<CoreState>;
}
