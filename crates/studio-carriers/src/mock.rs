//! In-memory target for tests: sparse RAM, optionally with an RTT control block.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::{CarrierError, Link, MemoryAccess, Result};

#[derive(Default)]
struct Inner {
    /// Byte-granular RAM; unset bytes read as zero
    ram: BTreeMap<u64, u8>,
    /// Reads touching `[start, end)` fail
    faults: Vec<(u64, u64)>,
    reads: u64,
    /// Control block address and ring size, once [`MockLink::init_rtt`] ran
    rtt: Option<(u64, u32)>,
}

impl Inner {
    fn poke(&mut self, address: u64, bytes: &[u8]) {
        for (i, b) in bytes.iter().enumerate() {
            self.ram.insert(address + i as u64, *b);
        }
    }

    fn word(&self, address: u64) -> u32 {
        let bytes: Vec<u8> = (0..4)
            .map(|i| self.ram.get(&(address + i)).copied().unwrap_or(0))
            .collect();
        u32::from_le_bytes(bytes.try_into().unwrap())
    }
}

/// Cloneable handle: the test keeps one to poke memory while the session owns another.
#[derive(Clone, Default)]
pub struct MockLink(Arc<Mutex<Inner>>);

impl MockLink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn poke(&self, address: u64, bytes: &[u8]) {
        self.0.lock().unwrap().poke(address, bytes);
    }

    pub fn peek(&self, address: u64, len: usize) -> Vec<u8> {
        let inner = self.0.lock().unwrap();
        (0..len as u64)
            .map(|i| inner.ram.get(&(address + i)).copied().unwrap_or(0))
            .collect()
    }

    /// Lay out an RTT control block at `address` with one up channel of `size` bytes,
    /// its name and ring placed right after the block.
    pub fn init_rtt(&self, address: u64, name: &str, size: u32) {
        let mut inner = self.0.lock().unwrap();
        let name_at = address + 0x100;
        let buffer_at = address + 0x200;
        let mut name_bytes = name.as_bytes().to_vec();
        name_bytes.push(0);
        inner.poke(name_at, &name_bytes);
        let mut block = b"SEGGER RTT\0\0\0\0\0\0".to_vec();
        for word in [1, 0, name_at as u32, buffer_at as u32, size, 0, 0, 0] {
            block.extend_from_slice(&u32::to_le_bytes(word));
        }
        inner.poke(address, &block);
        inner.rtt = Some((address, size));
    }

    /// Append to RTT up channel 0 as the firmware would; bytes beyond free space are dropped.
    pub fn push_log(&self, bytes: &[u8]) {
        let mut inner = self.0.lock().unwrap();
        let (block, size) = inner.rtt.expect("init_rtt first");
        let descriptor = block + 24;
        let buffer = inner.word(descriptor + 4) as u64;
        let mut write = inner.word(descriptor + 12);
        let read = inner.word(descriptor + 16);
        for &b in bytes {
            let next = (write + 1) % size;
            if next == read {
                break;
            }
            inner.poke(buffer + write as u64, &[b]);
            write = next;
        }
        inner.poke(descriptor + 12, &write.to_le_bytes());
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

impl Link for MockLink {
    fn with_memory(&mut self, body: &mut dyn FnMut(&mut dyn MemoryAccess)) -> Result<()> {
        body(self);
        Ok(())
    }
}
