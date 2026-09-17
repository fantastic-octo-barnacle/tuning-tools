//! SEGGER RTT up-channel reader over plain memory access.
//!
//! The target owns a control block (`_SEGGER_RTT`) describing ring buffers in
//! its RAM. The host reads pending bytes between the read and write offsets
//! and then advances the read offset; nothing halts the core. The channel's
//! mode flags are left as the firmware set them.

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
    /// Address of up-buffer descriptor 0
    descriptor: u64,
    buffer: u64,
    size: u32,
    name: String,
    blocking: bool,
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

        let mut offsets = [0u8; 8];
        memory.read(channel.descriptor + WR_OFF, &mut offsets)?;
        let write = u32::from_le_bytes(offsets[..4].try_into().unwrap());
        let read = u32::from_le_bytes(offsets[4..].try_into().unwrap());
        if write >= channel.size || read >= channel.size {
            // The firmware reset or overwrote the block: look for it again
            tracing::warn!(write, read, size = channel.size, "RTT offsets out of range");
            self.state = State::Searching {
                last_try: Some(now),
            };
            return Ok(0);
        }
        if write == read {
            return Ok(0);
        }
        let tail = if write > read {
            write - read
        } else {
            channel.size - read
        };
        let mut len = (tail as usize).min(buf.len());
        memory.read(channel.buffer + read as u64, &mut buf[..len])?;
        if write < read && len == tail as usize {
            // Wrapped: the rest starts at the beginning of the ring
            let head = (write as usize).min(buf.len() - len);
            if head > 0 {
                memory.read(channel.buffer, &mut buf[len..len + head])?;
            }
            len += head;
        }
        let next = (read as u64 + len as u64) % channel.size as u64;
        memory.write(channel.descriptor + RD_OFF, &(next as u32).to_le_bytes())?;
        Ok(len)
    }
}

fn find_channel(memory: &mut dyn MemoryAccess, address: u64) -> Result<Option<Channel>> {
    let mut block = [0u8; HEADER_LEN as usize + DESCRIPTOR_LEN];
    memory.read(address, &mut block)?;
    if !block.starts_with(ID) {
        return Ok(None);
    }
    let word = |at: usize| u32::from_le_bytes(block[at..at + 4].try_into().unwrap());
    let max_up = word(16);
    if max_up == 0 || max_up > 255 {
        return Ok(None);
    }
    let d = HEADER_LEN as usize;
    let (name_ptr, buffer, size) = (word(d), word(d + 4), word(d + 8));
    let flags = word(d + 20);
    if buffer == 0 || size == 0 {
        return Ok(None);
    }
    let name = read_name(memory, name_ptr as u64).unwrap_or_else(|| "up 0".into());
    Ok(Some(Channel {
        descriptor: address + HEADER_LEN,
        buffer: buffer as u64,
        size,
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
}
