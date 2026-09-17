//! SEGGER RTT channels over plain memory access.
//!
//! The target owns a control block (`_SEGGER_RTT`) describing ring buffers in
//! its RAM. The host reads an up channel's pending bytes between the read and
//! write offsets and then advances the read offset, and writes a down channel
//! by filling the ring after its write offset and then advancing that; nothing
//! halts the core. Channel mode flags are left as the firmware set them.
//!
//! [`RttReader`] drains up channel 0, the defmt log. [`RttDuplex`] carries a
//! framed protocol over a named up and down channel pair.

use std::time::{Duration, Instant};

use crate::{MemoryAccess, Result, StreamState};

const ID: &[u8] = b"SEGGER RTT\0";
/// `acID[16]`, `MaxNumUpBuffers`, `MaxNumDownBuffers`
const HEADER_LEN: u64 = 24;
/// `sName`, `pBuffer`, `SizeOfBuffer`, `WrOff`, `RdOff`, `Flags`
const DESCRIPTOR_LEN: usize = 24;
const WR_OFF: u64 = 12;
const RD_OFF: u64 = 16;
const MODE_MASK: u32 = 0b11;
const MODE_BLOCK_IF_FULL: u32 = 2;
/// How often to look for the control block before the firmware sets it up.
pub const RETRY: Duration = Duration::from_millis(500);

pub struct RttReader {
    address: u64,
    state: State,
}

enum State {
    Searching { last_try: Option<Instant> },
    Attached(Channel),
}

struct Channel {
    ring: Ring,
    name: String,
    blocking: bool,
}

/// One channel's ring buffer, located by its descriptor.
#[derive(Debug, Clone, Copy)]
struct Ring {
    descriptor: u64,
    buffer: u64,
    size: u32,
}

impl Ring {
    /// Copy pending bytes into `buf` and consume them. `None` when the offsets
    /// are out of range, which means the firmware reset or overwrote the block.
    fn read(&self, memory: &mut dyn MemoryAccess, buf: &mut [u8]) -> Result<Option<usize>> {
        let Some((write, read)) = self.offsets(memory)? else {
            return Ok(None);
        };
        if write == read {
            return Ok(Some(0));
        }
        let tail = if write > read {
            write - read
        } else {
            self.size - read
        };
        let mut len = (tail as usize).min(buf.len());
        memory.read(self.buffer + read as u64, &mut buf[..len])?;
        if write < read && len == tail as usize {
            // Wrapped: the rest starts at the beginning of the ring
            let head = (write as usize).min(buf.len() - len);
            if head > 0 {
                memory.read(self.buffer, &mut buf[len..len + head])?;
            }
            len += head;
        }
        let next = (read as u64 + len as u64) % self.size as u64;
        memory.write(self.descriptor + RD_OFF, &(next as u32).to_le_bytes())?;
        Ok(Some(len))
    }

    /// Append all of `bytes`, or none when they do not fit. `None` as for [`Ring::read`].
    fn write(&self, memory: &mut dyn MemoryAccess, bytes: &[u8]) -> Result<Option<bool>> {
        let Some((write, read)) = self.offsets(memory)? else {
            return Ok(None);
        };
        // One slot stays empty so a full ring is distinguishable from an empty one
        let free = if read > write {
            read - write - 1
        } else {
            self.size - write + read - 1
        };
        if bytes.len() > free as usize {
            return Ok(Some(false));
        }
        let (first, rest) = bytes.split_at(bytes.len().min((self.size - write) as usize));
        memory.write(self.buffer + write as u64, first)?;
        if !rest.is_empty() {
            memory.write(self.buffer, rest)?;
        }
        let next = (write as u64 + bytes.len() as u64) % self.size as u64;
        memory.write(self.descriptor + WR_OFF, &(next as u32).to_le_bytes())?;
        Ok(Some(true))
    }

    fn offsets(&self, memory: &mut dyn MemoryAccess) -> Result<Option<(u32, u32)>> {
        let mut offsets = [0u8; 8];
        memory.read(self.descriptor + WR_OFF, &mut offsets)?;
        let write = u32::from_le_bytes(offsets[..4].try_into().unwrap());
        let read = u32::from_le_bytes(offsets[4..].try_into().unwrap());
        if write >= self.size || read >= self.size {
            tracing::warn!(write, read, size = self.size, "RTT offsets out of range");
            return Ok(None);
        }
        Ok(Some((write, read)))
    }
}

impl RttReader {
    /// `address` is the control block, `_SEGGER_RTT` in the ELF.
    pub fn new(address: u64) -> Self {
        Self {
            address,
            state: State::Searching { last_try: None },
        }
    }

    pub fn state(&self) -> StreamState {
        match &self.state {
            State::Searching { .. } => StreamState::Searching,
            State::Attached(c) => StreamState::Attached {
                channel: c.name.clone(),
                blocking: c.blocking,
            },
        }
    }

    /// Copy pending bytes of up channel 0 into `buf`; `Ok(0)` when there are
    /// none or the control block is not set up yet. A short count means the
    /// channel is drained; call again while it fills `buf`.
    pub fn read(
        &mut self,
        memory: &mut dyn MemoryAccess,
        now: Instant,
        buf: &mut [u8],
    ) -> Result<usize> {
        let channel = match &mut self.state {
            State::Attached(channel) => channel,
            State::Searching { last_try } => {
                if last_try.is_some_and(|t| now.duration_since(t) < RETRY) {
                    return Ok(0);
                }
                *last_try = Some(now);
                // Before the firmware initialises it the block is zeroed or stale
                let Some(channel) = find_channel(memory, self.address)? else {
                    return Ok(0);
                };
                tracing::info!(channel = %channel.name, blocking = channel.blocking, "RTT attached");
                self.state = State::Attached(channel);
                let State::Attached(channel) = &mut self.state else {
                    unreachable!()
                };
                channel
            }
        };

        match channel.ring.read(memory, buf)? {
            Some(n) => Ok(n),
            None => {
                self.state = State::Searching {
                    last_try: Some(now),
                };
                Ok(0)
            }
        }
    }
}

/// A named up and down channel pair carrying a byte stream both ways.
pub struct RttDuplex {
    address: u64,
    up_name: &'static str,
    down_name: &'static str,
    last_try: Option<Instant>,
    rings: Option<(Ring, Ring)>,
}

impl RttDuplex {
    /// `address` is the control block; the names are the firmware's channel names.
    pub fn new(address: u64, up_name: &'static str, down_name: &'static str) -> Self {
        Self {
            address,
            up_name,
            down_name,
            last_try: None,
            rings: None,
        }
    }

    pub fn is_attached(&self) -> bool {
        self.rings.is_some()
    }

    /// Look for the channels, at most once per [`RETRY`]; `true` once found.
    /// A firmware without them keeps answering `false`.
    pub fn attach(&mut self, memory: &mut dyn MemoryAccess, now: Instant) -> Result<bool> {
        if self.rings.is_some() {
            return Ok(true);
        }
        if self.last_try.is_some_and(|t| now.duration_since(t) < RETRY) {
            return Ok(false);
        }
        self.last_try = Some(now);
        let Some(rings) = find_pair(memory, self.address, self.up_name, self.down_name)? else {
            return Ok(false);
        };
        tracing::info!(
            up = self.up_name,
            down = self.down_name,
            "RTT duplex attached"
        );
        self.rings = Some(rings);
        Ok(true)
    }

    /// Copy pending up-channel bytes into `buf`; `Ok(0)` when detached or empty.
    pub fn read(&mut self, memory: &mut dyn MemoryAccess, buf: &mut [u8]) -> Result<usize> {
        let Some((up, _)) = self.rings else {
            return Ok(0);
        };
        let n = up.read(memory, buf)?;
        if n.is_none() {
            self.rings = None;
        }
        Ok(n.unwrap_or(0))
    }

    /// Write all of `bytes` to the down channel; `false` when detached or the
    /// firmware has not made room for them yet.
    pub fn write(&mut self, memory: &mut dyn MemoryAccess, bytes: &[u8]) -> Result<bool> {
        let Some((_, down)) = self.rings else {
            return Ok(false);
        };
        let written = down.write(memory, bytes)?;
        if written.is_none() {
            self.rings = None;
        }
        Ok(written.unwrap_or(false))
    }
}

/// The control block's channel counts, when it is initialised.
fn read_header(memory: &mut dyn MemoryAccess, address: u64) -> Result<Option<(u32, u32)>> {
    let mut header = [0u8; HEADER_LEN as usize];
    memory.read(address, &mut header)?;
    if !header.starts_with(ID) {
        return Ok(None);
    }
    let word = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().unwrap());
    let (max_up, max_down) = (word(16), word(20));
    if max_up == 0 || max_up > 255 || max_down > 255 {
        return Ok(None);
    }
    Ok(Some((max_up, max_down)))
}

fn find_pair(
    memory: &mut dyn MemoryAccess,
    address: u64,
    up_name: &str,
    down_name: &str,
) -> Result<Option<(Ring, Ring)>> {
    let Some((max_up, max_down)) = read_header(memory, address)? else {
        return Ok(None);
    };
    let mut found = |first: u32, count: u32, name: &str| -> Result<Option<Ring>> {
        let mut table = vec![0u8; count as usize * DESCRIPTOR_LEN];
        let at = address + HEADER_LEN + u64::from(first) * DESCRIPTOR_LEN as u64;
        memory.read(at, &mut table)?;
        for (i, d) in table.as_chunks::<DESCRIPTOR_LEN>().0.iter().enumerate() {
            let word = |at: usize| u32::from_le_bytes(d[at..at + 4].try_into().unwrap());
            let (buffer, size) = (word(4), word(8));
            if buffer != 0
                && size != 0
                && read_name(memory, word(0).into()).as_deref() == Some(name)
            {
                return Ok(Some(Ring {
                    descriptor: at + (i * DESCRIPTOR_LEN) as u64,
                    buffer: buffer.into(),
                    size,
                }));
            }
        }
        Ok(None)
    };
    let Some(up) = found(0, max_up, up_name)? else {
        return Ok(None);
    };
    Ok(found(max_up, max_down, down_name)?.map(|down| (up, down)))
}

fn find_channel(memory: &mut dyn MemoryAccess, address: u64) -> Result<Option<Channel>> {
    if read_header(memory, address)?.is_none() {
        return Ok(None);
    }
    let mut block = [0u8; HEADER_LEN as usize + DESCRIPTOR_LEN];
    memory.read(address, &mut block)?;
    let word = |at: usize| u32::from_le_bytes(block[at..at + 4].try_into().unwrap());
    let d = HEADER_LEN as usize;
    let (name_ptr, buffer, size) = (word(d), word(d + 4), word(d + 8));
    let flags = word(d + 20);
    if buffer == 0 || size == 0 {
        return Ok(None);
    }
    let name = read_name(memory, name_ptr as u64).unwrap_or_else(|| "up 0".into());
    Ok(Some(Channel {
        ring: Ring {
            descriptor: address + HEADER_LEN,
            buffer: buffer as u64,
            size,
        },
        name,
        blocking: flags & MODE_MASK == MODE_BLOCK_IF_FULL,
    }))
}

fn read_name(memory: &mut dyn MemoryAccess, address: u64) -> Option<String> {
    if address == 0 {
        return None;
    }
    let mut raw = [0u8; 32];
    memory.read(address, &mut raw).ok()?;
    let end = raw.iter().position(|&b| b == 0)?;
    (end > 0).then(|| String::from_utf8_lossy(&raw[..end]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::MockLink;

    const BLOCK: u64 = 0x2000_0100;

    #[test]
    fn waits_for_the_control_block_then_drains_with_wraparound() {
        let mut mock = MockLink::new();
        let mut rtt = RttReader::new(BLOCK);
        let t0 = Instant::now();
        let mut buf = [0u8; 64];
        assert_eq!(rtt.read(&mut mock, t0, &mut buf).unwrap(), 0);
        assert_eq!(rtt.state(), StreamState::Searching);

        mock.init_rtt(BLOCK, "defmt", 8);
        // Retry is rate-limited
        assert_eq!(
            rtt.read(&mut mock, t0 + Duration::from_millis(10), &mut buf)
                .unwrap(),
            0
        );
        assert_eq!(rtt.state(), StreamState::Searching);
        let t1 = t0 + RETRY;
        assert_eq!(rtt.read(&mut mock, t1, &mut buf).unwrap(), 0);
        assert_eq!(
            rtt.state(),
            StreamState::Attached {
                channel: "defmt".into(),
                blocking: false
            }
        );

        mock.push_log(b"hello");
        let n = rtt.read(&mut mock, t1, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"hello");
        // 5 consumed; 6 more wrap past the end of the 8-byte ring
        mock.push_log(b"world!");
        let n = rtt.read(&mut mock, t1, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"world!");
        assert_eq!(rtt.read(&mut mock, t1, &mut buf).unwrap(), 0);

        // A small buffer takes it in pieces
        mock.push_log(b"abcdef");
        let mut small = [0u8; 4];
        let n = rtt.read(&mut mock, t1, &mut small).unwrap();
        assert_eq!(&small[..n], b"abcd");
        let n = rtt.read(&mut mock, t1, &mut small).unwrap();
        assert_eq!(&small[..n], b"ef");
    }

    #[test]
    fn searches_again_after_corrupt_offsets() {
        let mut mock = MockLink::new();
        mock.init_rtt(BLOCK, "defmt", 8);
        let mut rtt = RttReader::new(BLOCK);
        let now = Instant::now();
        let mut buf = [0u8; 8];
        rtt.read(&mut mock, now, &mut buf).unwrap();
        assert!(matches!(rtt.state(), StreamState::Attached { .. }));
        mock.poke(BLOCK + HEADER_LEN + WR_OFF, &100u32.to_le_bytes());
        assert_eq!(rtt.read(&mut mock, now, &mut buf).unwrap(), 0);
        assert_eq!(rtt.state(), StreamState::Searching);
    }

    #[test]
    fn a_duplex_finds_its_channels_by_name_and_moves_bytes_both_ways() {
        let mut mock = MockLink::new();
        let now = Instant::now();
        let mut link = RttDuplex::new(BLOCK, "telemetry", "control");
        mock.init_rtt(BLOCK, "defmt", 64);
        assert!(!link.attach(&mut mock, now).unwrap());

        mock.init_rtt_channels(BLOCK, &[("defmt", 64), ("telemetry", 8)], &[("control", 8)]);
        // Rate-limited like the log reader
        assert!(!link.attach(&mut mock, now).unwrap());
        assert!(link.attach(&mut mock, now + RETRY).unwrap());

        mock.push_log(b"log");
        mock.push_up(1, b"reply");
        let mut buf = [0u8; 16];
        let n = link.read(&mut mock, &mut buf).unwrap();
        assert_eq!(&buf[..n], b"reply");

        // Seven of eight slots are usable; a write that does not fit is refused whole
        assert!(link.write(&mut mock, b"abcde").unwrap());
        assert!(!link.write(&mut mock, b"fgh").unwrap());
        assert_eq!(mock.take_down(0), b"abcde");
        assert!(link.write(&mut mock, b"fghijkl").unwrap());
        assert_eq!(mock.take_down(0), b"fghijkl");
    }
}
