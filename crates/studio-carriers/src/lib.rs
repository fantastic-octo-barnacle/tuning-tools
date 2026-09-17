//! Carriers: how the studio reaches a target.
//!
//! A carrier gives random access to target memory ([`MemoryAccess`]). The log
//! stream (RTT) and the core run state are read through that same memory, so a
//! probe needs one open memory interface and nothing else. Every carrier is
//! owned by one hardware thread; the traits take `&mut self` and nothing here
//! is shared.

pub mod cortex_m;
pub mod mock;
pub mod probe;
pub mod rtt;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum StreamState {
    /// The firmware has no stream (no RTT control block in the ELF)
    Absent,
    /// Looking for the control block; firmware may not have initialised it yet
    Searching,
    Attached {
        channel: String,
        /// The firmware blocks when the buffer is full, so a slow host stalls it
        blocking: bool,
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

/// A connected target.
pub trait Link: Send {
    /// Run `body` with target memory held open.
    ///
    /// Opening memory can be expensive (a probe re-reads the access port's
    /// registers, several USB round trips), so callers stay inside `body` for as
    /// long as they can and only come back out to reopen after persistent errors.
    fn with_memory(&mut self, body: &mut dyn FnMut(&mut dyn MemoryAccess)) -> Result<()>;
}
