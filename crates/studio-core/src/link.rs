//! A session over the firmware's framed link (USB CDC or a UART).
//!
//! The firmware is the authority here: it sends its own catalog, checks every
//! write against its descriptors and the robot's state, and timestamps samples
//! with its own clock. The worker holds the tuning lease for as long as the
//! session runs and renews it well inside the firmware's timeout.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use studio_carriers::{ByteStream, CarrierError, CoreState, StreamState};

use crate::catalog::{widen, Access, Catalog, CatalogEntry, CellKind};
use crate::frame::FrameBuilder;
use crate::schedule::Ticker;
use crate::session::{
    LinkState, RequestReply, Session, SessionCommand, SessionEvent, SessionSink, FRAME_PERIOD,
    STATS_PERIOD, TUNE_PERIOD,
};
use crate::stats::LinkStats;
use crate::tune::{CatalogCheck, TuneValue};
use crate::wire::{self, cmd, Decoder, Frame, Reader};

pub const REPLY_TIMEOUT: Duration = Duration::from_millis(1000);
/// A save erases and programs a flash sector before it answers.
pub const SAVE_TIMEOUT: Duration = Duration::from_millis(3000);
const HELLO_ATTEMPTS: usize = 3;
const HELLO_TIMEOUT: Duration = Duration::from_millis(400);
const LEASE_RENEW: Duration = Duration::from_millis(1000);
/// Values per READ; the reply's 12 bytes each must fit a 256-byte payload
const READ_CHUNK: usize = 20;
const MAX_WATCHES: usize = 32;
const MAX_PERIOD_MS: u16 = 1000;
const PROTOCOL_VERSION: u8 = 1;

pub struct LinkOptions {
    pub rate_hz: f64,
}

/// Connect on a new thread and run until stopped or the link is lost.
pub fn spawn_link<C>(connect: C, options: LinkOptions, sink: Arc<dyn SessionSink>) -> Session
where
    C: FnOnce() -> Result<Box<dyn ByteStream>, CarrierError> + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("link-session".into())
        .spawn(move || {
            status(&*sink, LinkState::Connecting, None);
            let outcome = connect()
                .map_err(|e| e.to_string())
                .and_then(|stream| Worker::handshake(stream, options, sink.clone()));
            match outcome {
                Ok(worker) => worker.run(rx),
                Err(message) => status(&*sink, LinkState::Failed, Some(message)),
            }
        })
        .expect("spawn link session thread");
    Session::from_parts(tx, thread)
}

/// A lease token that differs between sessions and tools; never zero.
pub fn session_token() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() ^ std::process::id())
        .unwrap_or(1)
        | 1
}

fn status(sink: &dyn SessionSink, state: LinkState, message: Option<String>) {
    sink.event(SessionEvent::Status { state, message });
}

enum Pending {
    Lease,
    Read,
    Stats,
    Watch(Vec<Subscription>, u16),
    Write(RequestReply),
    Discard(RequestReply),
    Save(RequestReply),
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Subscription {
    watch_id: u32,
    kind: CellKind,
    cell: u32,
}

struct Worker {
    stream: Box<dyn ByteStream>,
    decoder: Decoder,
    sink: Arc<dyn SessionSink>,
    seq: u16,
    token: u32,
    catalog: Catalog,
    pending: HashMap<u16, (Pending, Instant)>,
    start: Instant,
    rate_hz: f64,
    /// Watches asked for, and the set the firmware is sampling
    wanted: Vec<(u32, u32)>,
    active: Vec<Subscription>,
    period_ms: u16,
    frame: FrameBuilder,
    row: Vec<f64>,
    /// Device time that maps to the session's time zero
    device_base_us: Option<i128>,
    last_sample_seq: Option<u16>,
    missed_samples: u64,
    values: HashMap<u32, TuneValue>,
    stats: LinkStats,
    device_dropped: u64,
    last_error: Option<String>,
}

impl Worker {
    fn handshake(
        stream: Box<dyn ByteStream>,
        options: LinkOptions,
        sink: Arc<dyn SessionSink>,
    ) -> Result<Self, String> {
        let token = session_token();
        let mut worker = Self {
            stream,
            decoder: Decoder::default(),
            sink,
            seq: 0,
            token,
            catalog: Catalog {
                address: 0,
                symbol: String::new(),
                entries: Vec::new(),
            },
            pending: HashMap::new(),
            start: Instant::now(),
            rate_hz: options.rate_hz,
            wanted: Vec::new(),
            active: Vec::new(),
            period_ms: 0,
            frame: FrameBuilder::default(),
            row: Vec::new(),
            device_base_us: None,
            last_sample_seq: None,
            missed_samples: 0,
            values: HashMap::new(),
            stats: LinkStats::default(),
            device_dropped: 0,
            last_error: None,
        };

        worker.sink.started(SystemTime::now());
        let hello = (0..HELLO_ATTEMPTS)
            .find_map(|_| worker.call(cmd::HELLO, &[], HELLO_TIMEOUT).transpose())
            .unwrap_or_else(|| {
                Err("the port is open but no rm-telemetry firmware answered on it".into())
            })?;
        let mut r = Reader::new(&hello);
        let version = r.u8().ok_or("the firmware's HELLO reply is too short")?;
        if version != PROTOCOL_VERSION {
            return Err(format!(
                "the firmware speaks protocol version {version}; this tool speaks {PROTOCOL_VERSION}"
            ));
        }

        worker
            .call(cmd::LEASE, &token.to_le_bytes(), REPLY_TIMEOUT)?
            .ok_or("the firmware did not answer the lease request")?;
        worker.catalog = worker.read_catalog()?;
        worker.catalog.symbol = "firmware catalog".into();
        status(&*worker.sink, LinkState::Connected, None);
        worker.sink.event(SessionEvent::Catalog {
            catalog: worker.catalog.clone(),
        });
        Ok(worker)
    }

    /// Send a request and wait for its reply's body. `Ok(None)` on timeout;
    /// `Err` when the firmware refused or the stream failed.
    fn call(
        &mut self,
        code: u8,
        payload: &[u8],
        timeout: Duration,
    ) -> Result<Option<Vec<u8>>, String> {
        let seq = self.send(code, payload)?;
        let until = Instant::now() + timeout;
        let mut buf = [0; 512];
        while Instant::now() < until {
            let n = self.stream.read(&mut buf).map_err(|e| e.to_string())?;
            let mut reply = None;
            self.decoder.feed(&buf[..n], |f| {
                if f.cmd == code | wire::REPLY && f.seq == seq {
                    reply = Some(f.payload);
                }
            });
            if let Some(payload) = reply {
                return match payload.split_first() {
                    Some((0, body)) => Ok(Some(body.to_vec())),
                    Some((&status, _)) => Err(wire::status_message(status).into()),
                    None => Err("the firmware sent an empty reply".into()),
                };
            }
        }
        Ok(None)
    }

    fn send(&mut self, code: u8, payload: &[u8]) -> Result<u16, String> {
        self.seq = self.seq.wrapping_add(1);
        let bytes = wire::encode(code, self.seq, payload);
        self.stream.write_all(&bytes).map_err(|e| e.to_string())?;
        Ok(self.seq)
    }

    fn request(&mut self, code: u8, payload: &[u8], pending: Pending) -> Result<(), String> {
        let seq = self.send(code, payload)?;
        self.pending.insert(seq, (pending, Instant::now()));
        Ok(())
    }

    fn read_catalog(&mut self) -> Result<Catalog, String> {
        let mut entries = Vec::new();
        loop {
            let offset = entries.len() as u16;
            let mut payload = offset.to_le_bytes().to_vec();
            payload.push(u8::MAX);
            let body = self
                .call(cmd::CATALOG, &payload, REPLY_TIMEOUT)?
                .ok_or("the firmware did not send its catalog")?;
            let mut r = Reader::new(&body);
            let short = "the firmware's catalog reply is malformed";
            let total = r.u16().ok_or(short)?;
            let returned = r.u8().ok_or(short)?;
            for _ in 0..returned {
                entries.push(parse_entry(&mut r).ok_or(short)?);
            }
            if entries.len() >= usize::from(total) {
                return Ok(Catalog {
                    address: 0,
                    symbol: String::new(),
                    entries,
                });
            }
            if returned == 0 {
                return Err(short.into());
            }
        }
    }

    fn run(mut self, rx: Receiver<SessionCommand>) {
        let now = Instant::now();
        let mut lease_tick = Ticker::new(now, LEASE_RENEW);
        let mut tune_tick = Ticker::new(now, TUNE_PERIOD);
        let mut frame_tick = Ticker::new(now, FRAME_PERIOD);
        let mut stats_tick = Ticker::new(now, STATS_PERIOD);
        let mut buf = [0; 4096];

        let failure = loop {
            match self.drain_commands(&rx) {
                Ok(true) => break None,
                Ok(false) => {}
                Err(e) => break Some(e),
            }
            let n = match self.stream.read(&mut buf) {
                Ok(n) => n,
                Err(e) => break Some(e.to_string()),
            };
            let mut frames = Vec::new();
            self.decoder.feed(&buf[..n], |f| frames.push(f));
            for f in frames {
                self.on_frame(f);
            }

            let now = Instant::now();
            let due = [
                lease_tick.poll(now),
                tune_tick.poll(now),
                stats_tick.poll(now),
            ];
            let sent = (|| {
                if due[0] {
                    self.request(cmd::LEASE, &self.token.to_le_bytes(), Pending::Lease)?;
                }
                if due[1] {
                    self.poll_values()?;
                }
                if due[2] {
                    self.request(cmd::STATS, &[], Pending::Stats)?;
                }
                Ok::<_, String>(())
            })();
            if let Err(e) = sent {
                break Some(e);
            }
            if frame_tick.poll(now) {
                self.flush_frame();
            }
            self.expire(now);
        };

        let _ = self.send(cmd::WATCH, &[0, 0, 0]);
        let _ = self.send(cmd::RELEASE, &self.token.to_le_bytes());
        self.flush_frame();
        for (_, (pending, _)) in self.pending.drain() {
            if let Pending::Write(reply) | Pending::Discard(reply) | Pending::Save(reply) = pending
            {
                let _ = reply.send(Err("the link closed".into()));
            }
        }
        let state = if failure.is_some() {
            LinkState::Failed
        } else {
            LinkState::Disconnected
        };
        status(&*self.sink, state, failure);
    }

    /// `Ok(true)` when the session should stop.
    fn drain_commands(&mut self, rx: &Receiver<SessionCommand>) -> Result<bool, String> {
        loop {
            let command = match rx.try_recv() {
                Ok(c) => c,
                Err(TryRecvError::Empty) => return Ok(false),
                Err(TryRecvError::Disconnected) => return Ok(true),
            };
            match command {
                SessionCommand::Stop => return Ok(true),
                SessionCommand::SetRate(hz) if hz.is_finite() && hz > 0.0 => {
                    self.rate_hz = hz;
                    self.subscribe(None)?;
                }
                SessionCommand::SetCellWatches(watches) => {
                    self.wanted = watches;
                    self.subscribe(None)?;
                }
                SessionCommand::Request { id, value, reply } => {
                    let payload = self.catalog.entry(id).map(|e| {
                        e.request_bits(value).map(|bits| {
                            let mut p = self.token.to_le_bytes().to_vec();
                            p.extend_from_slice(&id.to_le_bytes());
                            p.extend_from_slice(&wire::slot(e.kind.tag(), bits));
                            p
                        })
                    });
                    match payload {
                        Some(Ok(p)) => self.request(cmd::WRITE, &p, Pending::Write(reply))?,
                        Some(Err(e)) => {
                            let _ = reply.send(Err(e));
                        }
                        None => {
                            let _ = reply
                                .send(Err(format!("the firmware has no value with id {id:#010x}")));
                        }
                    }
                }
                SessionCommand::Discard { reply } => {
                    self.request(
                        cmd::DISCARD,
                        &self.token.to_le_bytes(),
                        Pending::Discard(reply),
                    )?;
                }
                SessionCommand::Save { reply } => {
                    self.request(cmd::SAVE, &self.token.to_le_bytes(), Pending::Save(reply))?;
                }
                SessionCommand::Read { reply, .. } => {
                    let _ = reply.send(Err("reading target memory needs a debug probe".into()));
                }
                SessionCommand::SetRate(_)
                | SessionCommand::SetWatches(_)
                | SessionCommand::SetCatalog(_) => {}
            }
        }
    }

    /// Ask the firmware to sample the wanted watches, at `period_ms` or the
    /// period the rate asks for.
    fn subscribe(&mut self, period_ms: Option<u16>) -> Result<(), String> {
        let subs: Vec<Subscription> = self
            .wanted
            .iter()
            .filter_map(|&(watch_id, cell)| {
                self.catalog.entry(cell).map(|e| Subscription {
                    watch_id,
                    kind: e.kind,
                    cell,
                })
            })
            .take(MAX_WATCHES)
            .collect();
        if self.wanted.len() > MAX_WATCHES {
            self.last_error = Some(format!(
                "the firmware samples at most {MAX_WATCHES} values; the rest are not plotted"
            ));
        }
        let period = period_ms.unwrap_or_else(|| {
            (1000.0 / self.rate_hz)
                .round()
                .clamp(1.0, f64::from(MAX_PERIOD_MS)) as u16
        });
        let (period, count) = if subs.is_empty() {
            (0, 0)
        } else {
            (period, subs.len() as u8)
        };
        let mut payload = period.to_le_bytes().to_vec();
        payload.push(count);
        for s in &subs {
            payload.extend_from_slice(&s.cell.to_le_bytes());
        }
        self.request(cmd::WATCH, &payload, Pending::Watch(subs, period))
    }

    fn poll_values(&mut self) -> Result<(), String> {
        let ids: Vec<u32> = self.catalog.entries.iter().map(|e| e.id).collect();
        for chunk in ids.chunks(READ_CHUNK) {
            let mut payload = vec![chunk.len() as u8];
            for id in chunk {
                payload.extend_from_slice(&id.to_le_bytes());
            }
            self.request(cmd::READ, &payload, Pending::Read)?;
        }
        let values = self
            .catalog
            .entries
            .iter()
            .filter_map(|e| self.values.get(&e.id).cloned())
            .collect();
        self.sink.event(SessionEvent::Tune {
            check: CatalogCheck::Matches,
            values,
        });
        Ok(())
    }

    fn on_frame(&mut self, f: Frame) {
        if f.cmd == cmd::SAMPLE {
            self.on_sample(f.seq, &f.payload);
            return;
        }
        let Some((pending, _)) = self.pending.remove(&f.seq) else {
            return;
        };
        let (status, body) = match f.payload.split_first() {
            Some((&status, body)) => (status, body),
            None => (1, &[][..]),
        };
        let refused = (status != 0).then(|| wire::status_message(status).to_string());
        match pending {
            Pending::Write(reply) | Pending::Discard(reply) | Pending::Save(reply) => {
                let _ = reply.send(refused.map_or(Ok(()), Err));
            }
            Pending::Watch(subs, period) => match status {
                0 => {
                    self.flush_frame();
                    self.frame.reset(subs.iter().map(|s| s.watch_id).collect());
                    self.active = subs;
                    self.period_ms = period;
                    self.last_sample_seq = None;
                    self.stats.restart_window();
                }
                wire::STATUS_BUDGET if period < MAX_PERIOD_MS => {
                    let slower = period.saturating_mul(2).min(MAX_PERIOD_MS);
                    self.last_error = Some(format!(
                        "the firmware's sample budget allows these watches only every {slower} ms"
                    ));
                    if let Err(e) = self.subscribe(Some(slower)) {
                        self.last_error = Some(e);
                    }
                }
                _ => self.last_error = refused,
            },
            Pending::Read if status == 0 => self.on_values(body),
            Pending::Stats if status == 0 => {
                let mut r = Reader::new(body);
                if let (Some(_sent), Some(dropped)) = (r.u32(), r.u32()) {
                    self.device_dropped = u64::from(dropped);
                }
                self.report_stats();
            }
            Pending::Lease if status == wire::STATUS_BUSY => self.last_error = refused,
            _ => {
                if refused.is_some() {
                    self.last_error = refused;
                }
            }
        }
    }

    fn on_values(&mut self, body: &[u8]) {
        let mut r = Reader::new(body);
        let Some(count) = r.u8() else {
            return;
        };
        for _ in 0..count {
            let (Some(id), Some(requested), Some(applied)) = (r.u32(), r.u32(), r.u32()) else {
                return;
            };
            let Some(entry) = self.catalog.entry(id) else {
                continue;
            };
            self.values.insert(
                id,
                TuneValue {
                    id,
                    requested: Some(entry.kind.decode(requested)),
                    applied: Some(entry.kind.decode(applied)),
                },
            );
        }
    }

    fn on_sample(&mut self, seq: u16, payload: &[u8]) {
        let mut r = Reader::new(payload);
        let (Some(device_us), Some(count)) = (r.u64(), r.u8()) else {
            return;
        };
        if usize::from(count) != self.active.len() || r.remaining() != self.active.len() * 4 {
            // Frames for a subscription the firmware has not confirmed yet
            return;
        }
        if let Some(last) = self.last_sample_seq {
            self.missed_samples += u64::from(seq.wrapping_sub(last).wrapping_sub(1));
        }
        self.last_sample_seq = Some(seq);
        let now = Instant::now();
        let elapsed_us = (now - self.start).as_micros() as i128;
        let base = *self
            .device_base_us
            .get_or_insert(i128::from(device_us) - elapsed_us);
        let time = (i128::from(device_us) - base) as f64 / 1e6;
        self.row.clear();
        for s in &self.active {
            let bits = r.u32().unwrap_or_default();
            self.row.push(s.kind.decode(bits));
        }
        self.frame.push(time, &self.row);
        self.stats
            .record_tick(now, Duration::ZERO, payload.len() as u64 + 10, 0);
    }

    fn flush_frame(&mut self) {
        if let Some(batch) = self.frame.take() {
            let batch = Arc::new(batch);
            self.sink.samples(&batch);
            if self.sink.frame(batch.encode()) {
                self.stats.frames_sent += 1;
            } else {
                self.stats.frames_dropped += 1;
            }
        }
    }

    fn report_stats(&mut self) {
        let target_hz = if self.period_ms == 0 {
            0.0
        } else {
            1000.0 / f64::from(self.period_ms)
        };
        let mut stats = self.stats.snapshot(
            target_hz,
            self.device_dropped + self.missed_samples,
            0,
            self.active.len(),
        );
        stats.failed_regions = self.decoder.bad_frames;
        self.sink.event(SessionEvent::Stats {
            stats,
            core: CoreState::Running,
            log: StreamState::Absent,
            last_error: self.last_error.take(),
        });
    }

    fn expire(&mut self, now: Instant) {
        let late: Vec<u16> = self
            .pending
            .iter()
            .filter(|(_, (pending, sent))| {
                let limit = if matches!(pending, Pending::Save(_)) {
                    SAVE_TIMEOUT
                } else {
                    REPLY_TIMEOUT
                };
                now - *sent > limit
            })
            .map(|(&seq, _)| seq)
            .collect();
        for seq in late {
            if let Some((pending, _)) = self.pending.remove(&seq) {
                match pending {
                    Pending::Write(reply) | Pending::Discard(reply) | Pending::Save(reply) => {
                        let _ = reply.send(Err("the firmware did not answer in time".into()));
                    }
                    Pending::Lease => {
                        self.last_error = Some("the firmware did not renew the lease".into());
                    }
                    Pending::Watch(..) | Pending::Read | Pending::Stats => {}
                }
            }
        }
    }
}

fn parse_entry(r: &mut Reader<'_>) -> Option<CatalogEntry> {
    let id = r.u32()?;
    let kind = CellKind::from_tag(r.u8()?)?;
    let access = Access::from_tag(u64::from(r.u8()?))?;
    let default = r.u32()?;
    let [min, max, step] = [r.u32()?, r.u32()?, r.u32()?].map(|b| widen(f32::from_bits(b)));
    let name = r.text()?;
    let unit = r.text()?;
    let tunable = access != Access::ReadOnly;
    Some(CatalogEntry {
        id,
        name,
        unit,
        kind,
        access,
        default: kind.decode(default),
        min: tunable.then_some(min),
        max: tunable.then_some(max),
        max_step: tunable.then_some(step),
        requested_address: 0,
        applied_address: 0,
    })
}
