//! Tuning requests over the firmware's RTT control channel, for a probe session.
//!
//! A firmware that serves rm-telemetry over RTT gets the same requests a
//! framed link sends: the firmware checks each write against its descriptors
//! and the robot's state, and SAVE writes its flash. Samples still come from
//! memory reads. The lease is taken with the first request and renewed from
//! then on, so a probe session that only watches never blocks a USB tool.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use studio_carriers::rtt::RttDuplex;
use studio_carriers::MemoryAccess;

use crate::link::{session_token, REPLY_TIMEOUT, SAVE_TIMEOUT};
use crate::session::RequestReply;
use crate::wire::{self, cmd, Decoder};

pub const UP_CHANNEL: &str = "telemetry";
pub const DOWN_CHANNEL: &str = "control";
const LEASE_RENEW: Duration = Duration::from_millis(1000);
const STATUS_LEASE_LOST: u8 = 8;

#[derive(Debug, Clone, Copy)]
pub enum FramedRequest {
    Write { id: u32, tag: u8, bits: u32 },
    Discard,
    Save,
}

struct Pending {
    /// `None` for a lease renewal, which nobody waits on
    reply: Option<RequestReply>,
    deadline: Instant,
}

pub struct RttTuning {
    duplex: RttDuplex,
    decoder: Decoder,
    seq: u16,
    token: u32,
    /// When the lease was last asked for; `None` until the first request
    leased_at: Option<Instant>,
    pending: HashMap<u16, Pending>,
    buf: Vec<u8>,
}

impl RttTuning {
    /// `address` is the RTT control block.
    pub fn new(address: u64) -> Self {
        Self {
            duplex: RttDuplex::new(address, UP_CHANNEL, DOWN_CHANNEL),
            decoder: Decoder::default(),
            seq: 0,
            token: session_token(),
            leased_at: None,
            pending: HashMap::new(),
            buf: vec![0; 1024],
        }
    }

    /// Look for the channels, rate-limited; `true` once the firmware has them.
    pub fn attach(&mut self, memory: &mut dyn MemoryAccess, now: Instant) -> bool {
        self.duplex.attach(memory, now).unwrap_or(false)
    }

    /// Send `request`; `reply` is answered when the firmware replies or the
    /// request times out. Call only once [`RttTuning::attach`] succeeded.
    pub fn submit(
        &mut self,
        memory: &mut dyn MemoryAccess,
        now: Instant,
        request: FramedRequest,
        reply: RequestReply,
    ) {
        if self.leased_at.is_none() {
            if let Err(e) = self.lease(memory, now) {
                let _ = reply.send(Err(e));
                return;
            }
        }
        let token = self.token.to_le_bytes();
        let (code, payload, timeout) = match request {
            FramedRequest::Write { id, tag, bits } => {
                let mut p = token.to_vec();
                p.extend_from_slice(&id.to_le_bytes());
                p.extend_from_slice(&wire::slot(tag, bits));
                (cmd::WRITE, p, REPLY_TIMEOUT)
            }
            FramedRequest::Discard => (cmd::DISCARD, token.to_vec(), REPLY_TIMEOUT),
            FramedRequest::Save => (cmd::SAVE, token.to_vec(), SAVE_TIMEOUT),
        };
        if let Err(e) = self.send(memory, now, code, &payload, timeout, Some(reply.clone())) {
            let _ = reply.send(Err(e));
        }
    }

    /// Renew the lease when due, collect replies and time out late ones.
    /// Returns whether any request finished, and the last error seen.
    pub fn poll(&mut self, memory: &mut dyn MemoryAccess, now: Instant) -> (bool, Option<String>) {
        let mut error = None;
        if self.leased_at.is_some_and(|at| now - at >= LEASE_RENEW) {
            if let Err(e) = self.lease(memory, now) {
                error = Some(e);
            }
        }
        let mut finished = false;
        if !self.pending.is_empty() {
            loop {
                let n = match self.duplex.read(memory, &mut self.buf) {
                    Ok(n) => n,
                    Err(e) => {
                        error = Some(e.to_string());
                        break;
                    }
                };
                let mut frames = Vec::new();
                self.decoder.feed(&self.buf[..n], |f| frames.push(f));
                for f in frames {
                    let Some(pending) = self.pending.remove(&f.seq) else {
                        continue;
                    };
                    let status = f.payload.first().copied().unwrap_or(1);
                    if status == STATUS_LEASE_LOST
                        || (f.cmd == cmd::LEASE | wire::REPLY && status != 0)
                    {
                        // The firmware reset or another tool took over: ask again next time
                        self.leased_at = None;
                    }
                    let result = match status {
                        0 => Ok(()),
                        s => Err(wire::status_message(s).to_string()),
                    };
                    match pending.reply {
                        Some(reply) => {
                            finished = true;
                            let _ = reply.send(result);
                        }
                        None => error = result.err().or(error),
                    }
                }
                if n < self.buf.len() {
                    break;
                }
            }
        }
        let late: Vec<u16> = self
            .pending
            .iter()
            .filter(|(_, p)| now > p.deadline)
            .map(|(&seq, _)| seq)
            .collect();
        for seq in late {
            if let Some(Pending {
                reply: Some(reply), ..
            }) = self.pending.remove(&seq)
            {
                finished = true;
                let _ = reply.send(Err("the firmware did not answer in time".into()));
            }
        }
        (finished, error)
    }

    /// Give up the lease and fail whatever is still waiting.
    pub fn close(&mut self, memory: Option<&mut dyn MemoryAccess>) {
        if let (Some(memory), Some(_)) = (memory, self.leased_at) {
            let payload = self.token.to_le_bytes();
            let frame = self.frame(cmd::RELEASE, &payload);
            let _ = self.duplex.write(memory, &frame);
        }
        self.leased_at = None;
        for (_, pending) in self.pending.drain() {
            if let Some(reply) = pending.reply {
                let _ = reply.send(Err("the session closed".into()));
            }
        }
    }

    fn lease(&mut self, memory: &mut dyn MemoryAccess, now: Instant) -> Result<(), String> {
        let payload = self.token.to_le_bytes();
        self.send(memory, now, cmd::LEASE, &payload, REPLY_TIMEOUT, None)?;
        self.leased_at = Some(now);
        Ok(())
    }

    fn frame(&mut self, code: u8, payload: &[u8]) -> Vec<u8> {
        self.seq = self.seq.wrapping_add(1);
        wire::encode(code, self.seq, payload)
    }

    fn send(
        &mut self,
        memory: &mut dyn MemoryAccess,
        now: Instant,
        code: u8,
        payload: &[u8],
        timeout: Duration,
        reply: Option<RequestReply>,
    ) -> Result<(), String> {
        let bytes = self.frame(code, payload);
        match self.duplex.write(memory, &bytes) {
            Ok(true) => {
                self.pending.insert(
                    self.seq,
                    Pending {
                        reply,
                        deadline: now + timeout,
                    },
                );
                Ok(())
            }
            Ok(false) => Err("the firmware's RTT control channel is full; try again".into()),
            Err(e) => Err(e.to_string()),
        }
    }
}
