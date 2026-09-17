//! In-memory target for tests: sparse RAM plus a scripted log stream.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use crate::{ByteStream, CarrierError, CoreState, Link, MemoryAccess, Result, StreamState};

#[derive(Default)]
struct Inner {
    /// Byte-granular RAM; unset bytes read as zero
    ram: BTreeMap<u64, u8>,
    log: VecDeque<u8>,
    /// Reads touching `[start, end)` fail
    faults: Vec<(u64, u64)>,
    reads: u64,
}

/// Cloneable handle: the test keeps one to poke memory while the session owns another.
#[derive(Clone, Default)]
pub struct MockLink(Arc<Mutex<Inner>>);

impl MockLink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn poke(&self, address: u64, bytes: &[u8]) {
        let mut inner = self.0.lock().unwrap();
        for (i, b) in bytes.iter().enumerate() {
            inner.ram.insert(address + i as u64, *b);
        }
    }

    pub fn peek(&self, address: u64, len: usize) -> Vec<u8> {
        let inner = self.0.lock().unwrap();
        (0..len as u64)
            .map(|i| inner.ram.get(&(address + i)).copied().unwrap_or(0))
            .collect()
    }

    pub fn push_log(&self, bytes: &[u8]) {
        self.0.lock().unwrap().log.extend(bytes);
    }

    pub fn fail_reads(&self, start: u64, end: u64) {
        self.0.lock().unwrap().faults.push((start, end));
    }

    /// Number of `MemoryAccess::read` calls so far
    pub fn read_calls(&self) -> u64 {
        self.0.lock().unwrap().reads
    }
}

impl MemoryAccess for MockLink {
    fn read(&mut self, address: u64, buf: &mut [u8]) -> Result<()> {
        let mut inner = self.0.lock().unwrap();
        inner.reads += 1;
        let end = address + buf.len() as u64;
        if inner.faults.iter().any(|&(s, e)| address < e && s < end) {
            return Err(CarrierError::Read {
                address,
                len: buf.len(),
                reason: "mock fault".into(),
            });
        }
        for (i, b) in buf.iter_mut().enumerate() {
            *b = inner.ram.get(&(address + i as u64)).copied().unwrap_or(0);
        }
        Ok(())
    }

    fn write(&mut self, address: u64, data: &[u8]) -> Result<()> {
        self.poke(address, data);
        Ok(())
    }
}

impl ByteStream for MockLink {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let mut inner = self.0.lock().unwrap();
        let n = buf.len().min(inner.log.len());
        for (slot, b) in buf.iter_mut().zip(inner.log.drain(..n)) {
            *slot = b;
        }
        Ok(n)
    }

    fn state(&self) -> StreamState {
        StreamState::Attached {
            channel: "mock".into(),
        }
    }
}

impl Link for MockLink {
    fn memory(&mut self) -> &mut dyn MemoryAccess {
        self
    }

    fn log(&mut self) -> &mut dyn ByteStream {
        self
    }

    fn core_state(&mut self) -> Result<CoreState> {
        Ok(CoreState::Running)
    }
}
