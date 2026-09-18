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
    /// Control block address and up channel count, once [`MockLink::init_rtt`] ran
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

    /// An RTT control block with one up channel, as a defmt-only firmware has.
    pub fn init_rtt(&self, address: u64, name: &str, size: u32) {
        self.init_rtt_channels(address, &[(name, size)], &[]);
    }

    /// An RTT control block with the given `(name, size)` up and down channels.
    pub fn init_rtt_channels(&self, address: u64, up: &[(&str, u32)], down: &[(&str, u32)]) {
        let mut inner = self.0.lock().unwrap();
        let mut block = b"SEGGER RTT\0\0\0\0\0\0".to_vec();
        block.extend_from_slice(&(up.len() as u32).to_le_bytes());
        block.extend_from_slice(&(down.len() as u32).to_le_bytes());
        let mut buffer_at = address + 0x1000;
        for (i, (name, size)) in up.iter().chain(down).enumerate() {
            let name_at = address + 0x800 + 0x20 * i as u64;
            let mut name_bytes = name.as_bytes().to_vec();
            name_bytes.push(0);
            inner.poke(name_at, &name_bytes);
            for word in [name_at as u32, buffer_at as u32, *size, 0, 0, 0] {
                block.extend_from_slice(&word.to_le_bytes());
            }
            buffer_at += u64::from(*size) + 0x100;
        }
        inner.poke(address, &block);
        inner.rtt = Some((address, up.len() as u32));
    }

    /// Append to RTT up channel 0 as the firmware would; bytes beyond free space are dropped.
    pub fn push_log(&self, bytes: &[u8]) {
        self.push_up(0, bytes);
    }

    /// Append to RTT up channel `index`; bytes beyond free space are dropped.
    pub fn push_up(&self, index: u32, bytes: &[u8]) {
        let mut inner = self.0.lock().unwrap();
        let (block, _) = inner.rtt.expect("init_rtt first");
        let descriptor = block + 24 + 24 * u64::from(index);
        let buffer = inner.word(descriptor + 4) as u64;
        let size = inner.word(descriptor + 8);
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

    /// Consume everything the host wrote to RTT down channel `index`.
    pub fn take_down(&self, index: u32) -> Vec<u8> {
        let mut inner = self.0.lock().unwrap();
        let (block, up) = inner.rtt.expect("init_rtt first");
        let descriptor = block + 24 + 24 * u64::from(up + index);
        let buffer = inner.word(descriptor + 4) as u64;
        let size = inner.word(descriptor + 8);
        let write = inner.word(descriptor + 12);
        let mut read = inner.word(descriptor + 16);
        let mut out = Vec::new();
        while read != write {
            out.push(
                inner
                    .ram
                    .get(&(buffer + u64::from(read)))
                    .copied()
                    .unwrap_or(0),
            );
            read = (read + 1) % size;
        }
        inner.poke(descriptor + 16, &read.to_le_bytes());
        out
    }

    pub fn fail_reads(&self, start: u64, end: u64) {
        self.0.lock().unwrap().faults.push((start, end));
    }

    /// Let every read succeed again
    pub fn clear_faults(&self) {
        self.0.lock().unwrap().faults.clear();
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
