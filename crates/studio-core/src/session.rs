//! The hardware owner: one thread per connection owns the carrier, samples the
//! watched set on absolute deadlines, pumps the log stream and reports status.
//! Everything else talks to it through [`SessionCommand`]s.

use std::sync::mpsc::{self, Sender, SyncSender, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use studio_carriers::rtt::RttReader;
use studio_carriers::{cortex_m, CarrierError, CoreState, Link, MemoryAccess, StreamState};

use crate::catalog::{Catalog, TableLayout};
use crate::frame::{FrameBuilder, SampleBatch};
use crate::log::{LogDecoder, LogLine};
use crate::plan::{ReadItem, ReadPlan};
use crate::rtt_tuning::{FramedRequest, RttTuning};
use crate::schedule::{sleep_until, Ticker};
use crate::stats::{LinkStats, StatsSnapshot};
use crate::tune::{CatalogCheck, TuneValue, Tuner};

pub const FRAME_PERIOD: Duration = Duration::from_millis(33);
pub const LOG_PERIOD: Duration = Duration::from_millis(10);
pub const STATS_PERIOD: Duration = Duration::from_millis(500);
/// Tune panel refresh; people read these, they do not plot them
pub const TUNE_PERIOD: Duration = Duration::from_millis(100);
pub const MAX_RATE_HZ: f64 = 2000.0;
/// Every read failing for this long means the link is gone.
pub const LINK_LOST_AFTER: Duration = Duration::from_secs(2);
/// Longest a command waits while the worker has nothing due.
const COMMAND_POLL: Duration = Duration::from_millis(10);
/// Every read failing for this long reopens target memory, which recovers a
/// probe whose access port lost its state.
pub const REOPEN_AFTER: Duration = Duration::from_millis(250);

pub type RequestReply = SyncSender<Result<(), String>>;
type PendingRequest = (TuneRequest, RequestReply);
/// Bytes of each region read, in order
pub type ReadReply = SyncSender<Result<Vec<Vec<u8>>, String>>;
type PendingRead = (Vec<(u64, usize)>, ReadReply);

#[derive(Debug, Clone, Copy)]
enum TuneRequest {
    Set { id: u32, value: f64 },
    Discard,
    Save,
}

pub enum SessionCommand {
    SetWatches(Vec<ReadItem>),
    SetRate(f64),
    /// The open ELF's tuning table; `None` when it declares none
    SetCatalog(Option<Box<(TableLayout, Catalog)>>),
    /// Ask the firmware to run catalog value `id` at `value`
    Request {
        id: u32,
        value: f64,
        reply: RequestReply,
    },
    /// Ask for every tunable's built-in default
    Discard {
        reply: RequestReply,
    },
    /// Ask the firmware to store every current request so it survives a
    /// power cycle; needs a framed link or a firmware serving one over RTT
    Save {
        reply: RequestReply,
    },
    /// Sample tuning table values by id, as `(watch id, value id)`; used by a
    /// framed link, where values have no address. A probe session ignores it.
    SetCellWatches(Vec<(u32, u32)>),
    /// Read target memory once, as `(address, length)` regions; needs a probe
    Read {
        regions: Vec<(u64, usize)>,
        reply: ReadReply,
    },
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkState {
    Connecting,
    Connected,
    Disconnected,
    Failed,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum SessionEvent {
    #[serde(rename_all = "camelCase")]
    Status {
        state: LinkState,
        message: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Stats {
        #[serde(flatten)]
        stats: StatsSnapshot,
        core: CoreState,
        log: StreamState,
        /// Most recent read error in this period, if any
        last_error: Option<String>,
    },
    Log {
        lines: Vec<LogLine>,
    },
    Tune {
        check: CatalogCheck,
        values: Vec<TuneValue>,
    },
    /// The catalog a framed link's firmware sent; a probe session never sends it
    Catalog {
        catalog: Catalog,
    },
}

/// Where a session delivers its output. Called from the session threads.
pub trait SessionSink: Send + Sync + 'static {
    /// Deliver an encoded sample frame; `false` when it could not be delivered.
    fn frame(&self, bytes: Vec<u8>) -> bool;
    fn event(&self, event: SessionEvent);
    /// Every flushed batch, before it is encoded for [`SessionSink::frame`].
    /// Must not block: it runs on the sampling thread.
    fn samples(&self, _batch: &Arc<SampleBatch>) {}
    /// The wall-clock time of the session's time zero, once it is taken.
    fn started(&self, _at: SystemTime) {}
}

pub struct SessionOptions {
    pub rate_hz: f64,
    /// ELF bytes for the defmt table; `None` disables log decoding
    pub elf: Option<Vec<u8>>,
    /// RTT control block (`_SEGGER_RTT`); the log is read from its up channel
    /// 0, and tuning requests go through its control channels when it has them
    pub rtt_address: Option<u64>,
}

pub struct Session {
    tx: Sender<SessionCommand>,
    thread: Option<JoinHandle<()>>,
}

impl Session {
    /// Connect on a new thread and run until stopped or the link is lost.
    pub fn spawn<C>(connect: C, options: SessionOptions, sink: Arc<dyn SessionSink>) -> Self
    where
        C: FnOnce() -> Result<Box<dyn Link>, CarrierError> + Send + 'static,
    {
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("session".into())
            .spawn(move || {
                sink.event(SessionEvent::Status {
                    state: LinkState::Connecting,
                    message: None,
                });
                match connect() {
                    Ok(link) => Worker::new(options, sink).run(link, rx),
                    Err(e) => sink.event(SessionEvent::Status {
                        state: LinkState::Failed,
                        message: Some(e.to_string()),
                    }),
                }
            })
            .expect("spawn session thread");
        Self {
            tx,
            thread: Some(thread),
        }
    }

    pub(crate) fn from_parts(tx: Sender<SessionCommand>, thread: JoinHandle<()>) -> Self {
        Self {
            tx,
            thread: Some(thread),
        }
    }

    /// `false` when the session has already ended.
    pub fn send(&self, command: SessionCommand) -> bool {
        self.tx.send(command).is_ok()
    }

    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.tx.send(SessionCommand::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Why the worker came out of target memory.
enum Exit {
    Stop,
    Lost(String),
    Reopen,
}

struct Worker {
    sink: Arc<dyn SessionSink>,
    start: Instant,
    rate_hz: f64,
    plan: ReadPlan,
    sampler: Option<Ticker>,
    skipped_before: u64,
    frame: FrameBuilder,
    row: Vec<f64>,
    stats: LinkStats,
    decoder: Option<LogDecoder>,
    rtt: Option<RttReader>,
    log_buf: Vec<u8>,
    last_error: Option<String>,
    failing_since: Option<Instant>,
    stopping: bool,
    log_tick: Ticker,
    frame_tick: Ticker,
    stats_tick: Ticker,
    tuner: Option<Tuner>,
    tune_tick: Ticker,
    requests: Vec<PendingRequest>,
    reads: Vec<PendingRead>,
    framed: Option<RttTuning>,
}

impl Worker {
    fn new(options: SessionOptions, sink: Arc<dyn SessionSink>) -> Self {
        let start = Instant::now();
        sink.started(SystemTime::now());
        let decoder = options.elf.and_then(|elf| {
            let log_sink = sink.clone();
            match LogDecoder::spawn(elf, move |lines| {
                log_sink.event(SessionEvent::Log { lines })
            }) {
                Ok(decoder) => decoder,
                Err(message) => {
                    tracing::warn!(%message, "defmt decoding disabled");
                    None
                }
            }
        });
        let rtt = decoder
            .as_ref()
            .and(options.rtt_address)
            .map(RttReader::new);
        let framed = options.rtt_address.map(RttTuning::new);
        Self {
            sink,
            start,
            rate_hz: options.rate_hz.clamp(1.0, MAX_RATE_HZ),
            plan: ReadPlan::default(),
            sampler: None,
            skipped_before: 0,
            frame: FrameBuilder::default(),
            row: Vec::new(),
            stats: LinkStats::default(),
            decoder,
            rtt,
            log_buf: vec![0; 4096],
            last_error: None,
            failing_since: None,
            stopping: false,
            log_tick: Ticker::new(start, LOG_PERIOD),
            frame_tick: Ticker::new(start, FRAME_PERIOD),
            stats_tick: Ticker::new(start + STATS_PERIOD, STATS_PERIOD),
            tuner: None,
            tune_tick: Ticker::new(start, TUNE_PERIOD),
            requests: Vec::new(),
            reads: Vec::new(),
            framed,
        }
    }

    fn run(mut self, mut link: Box<dyn Link>, rx: mpsc::Receiver<SessionCommand>) {
        self.sink.event(SessionEvent::Status {
            state: LinkState::Connected,
            message: None,
        });
        let failure = loop {
            let mut exit = Exit::Reopen;
            let opened = link.with_memory(&mut |memory| exit = self.serve(memory, &rx));
            match (opened, exit) {
                (_, Exit::Stop) => break None,
                (_, Exit::Lost(message)) => break Some(message),
                (Ok(()), Exit::Reopen) => {}
                (Err(e), Exit::Reopen) => {
                    // Memory would not open: wait a little, still answering commands
                    let now = Instant::now();
                    let since = *self.failing_since.get_or_insert(now);
                    if now - since >= LINK_LOST_AFTER {
                        break Some(e.to_string());
                    }
                    self.last_error = Some(e.to_string());
                    let retry = now + REOPEN_AFTER;
                    while Instant::now() < retry {
                        sleep_until((Instant::now() + COMMAND_POLL).min(retry));
                        if self.drain_commands(&rx) {
                            break;
                        }
                        for (_, reply) in self.requests.drain(..) {
                            let _ = reply.send(Err("the target's memory is not open".into()));
                        }
                        for (_, reply) in self.reads.drain(..) {
                            let _ = reply.send(Err("the target's memory is not open".into()));
                        }
                    }
                    if self.stopping {
                        break None;
                    }
                }
            }
        };

        if let Some(framed) = &mut self.framed {
            framed.close(None);
        }
        self.flush_frame();
        self.sink.event(SessionEvent::Status {
            state: if failure.is_some() {
                LinkState::Failed
            } else {
                LinkState::Disconnected
            },
            message: failure,
        });
    }

    /// The session loop, with target memory held open.
    fn serve(
        &mut self,
        memory: &mut dyn MemoryAccess,
        rx: &mpsc::Receiver<SessionCommand>,
    ) -> Exit {
        let opened = Instant::now();
        loop {
            let mut next = self
                .log_tick
                .deadline()
                .min(self.frame_tick.deadline())
                .min(self.stats_tick.deadline());
            if self.tuner.is_some() {
                next = next.min(self.tune_tick.deadline());
            }
            if let Some(s) = &self.sampler {
                next = next.min(s.deadline());
            }
            // A thread sleep, not `recv_timeout`: channel timeouts round up to the
            // 15.6 ms system tick on Windows, which would cap sampling near 64 Hz
            sleep_until(next);
            if self.drain_commands(rx) {
                if let Some(framed) = &mut self.framed {
                    framed.close(Some(memory));
                }
                return Exit::Stop;
            }
            let mut wrote = self.serve_requests(memory);
            self.serve_reads(memory);

            let now = Instant::now();
            if self.sampler.as_mut().is_some_and(|s| s.poll(now)) {
                if let Some(message) = self.sample(memory, now) {
                    return Exit::Lost(message);
                }
                if self
                    .failing_since
                    .is_some_and(|since| now - since.max(opened) >= REOPEN_AFTER)
                {
                    return Exit::Reopen;
                }
            }
            if self.log_tick.poll(now) {
                self.pump_log(memory, now);
                if let Some(framed) = &mut self.framed {
                    framed.attach(memory, now);
                    let (finished, error) = framed.poll(memory, now);
                    wrote |= finished;
                    if error.is_some() {
                        self.last_error = error;
                    }
                }
            }
            if self.frame_tick.poll(now) {
                self.flush_frame();
            }
            if self.stats_tick.poll(now) {
                self.report_stats(memory);
            }
            // After a write, report at once so the panel shows the request land
            if self.tune_tick.poll(now) || wrote {
                self.report_tune(memory);
            }
        }
    }

    /// Apply every queued command; `true` when the session should stop.
    fn drain_commands(&mut self, rx: &mpsc::Receiver<SessionCommand>) -> bool {
        loop {
            match rx.try_recv() {
                Ok(SessionCommand::Stop) | Err(TryRecvError::Disconnected) => {
                    self.stopping = true;
                    return true;
                }
                Ok(command) => self.apply(command),
                Err(TryRecvError::Empty) => return false,
            }
        }
    }

    fn apply(&mut self, command: SessionCommand) {
        match command {
            SessionCommand::SetWatches(items) => self.set_watches(items),
            SessionCommand::SetRate(hz) => self.set_rate(hz),
            SessionCommand::SetCatalog(catalog) => {
                self.tuner = catalog.map(|c| {
                    let (layout, catalog) = *c;
                    Tuner::new(layout, catalog)
                });
            }
            SessionCommand::Request { id, value, reply } => {
                self.requests.push((TuneRequest::Set { id, value }, reply))
            }
            SessionCommand::Discard { reply } => self.requests.push((TuneRequest::Discard, reply)),
            SessionCommand::Save { reply } => self.requests.push((TuneRequest::Save, reply)),
            SessionCommand::Read { regions, reply } => self.reads.push((regions, reply)),
            SessionCommand::SetCellWatches(_) => {}
            SessionCommand::Stop => {}
        }
    }

    fn set_watches(&mut self, items: Vec<ReadItem>) {
        self.flush_frame();
        self.frame.reset(items.iter().map(|i| i.id).collect());
        self.plan = ReadPlan::new(items);
        self.restart_sampler();
    }

    fn set_rate(&mut self, hz: f64) {
        if !hz.is_finite() {
            return;
        }
        self.rate_hz = hz.clamp(1.0, MAX_RATE_HZ);
        self.restart_sampler();
    }

    fn restart_sampler(&mut self) {
        self.skipped_before += self.sampler.as_ref().map_or(0, Ticker::skipped);
        self.sampler =
            (!self.plan.is_empty()).then(|| Ticker::from_hz(Instant::now(), self.rate_hz));
        self.stats.restart_window();
        self.failing_since = None;
    }

    /// Returns a message when the link should be considered lost.
    fn sample(&mut self, memory: &mut dyn MemoryAccess, now: Instant) -> Option<String> {
        let started = Instant::now();
        let outcome = self.plan.sample(memory, &mut self.row);
        let read = started.elapsed();
        self.stats
            .record_tick(now, read, outcome.bytes, outcome.regions_failed);
        let time = (started - self.start).as_secs_f64();
        self.frame.push(time, &self.row);

        if let Some(e) = outcome.first_error {
            self.last_error = Some(e);
        }
        if outcome.regions_ok == 0 && outcome.regions_failed > 0 {
            let since = *self.failing_since.get_or_insert(now);
            if now - since >= LINK_LOST_AFTER {
                return Some(format!(
                    "every read failed for {} s: {}",
                    LINK_LOST_AFTER.as_secs(),
                    self.last_error.as_deref().unwrap_or("unknown error")
                ));
            }
        } else {
            self.failing_since = None;
        }
        None
    }

    fn pump_log(&mut self, memory: &mut dyn MemoryAccess, now: Instant) {
        let (Some(decoder), Some(rtt)) = (&self.decoder, &mut self.rtt) else {
            return;
        };
        // Drain what is buffered, bounded so a chatty target cannot starve sampling
        for _ in 0..8 {
            match rtt.read(memory, now, &mut self.log_buf) {
                Ok(0) => break,
                Ok(n) => {
                    self.stats.log_bytes += n as u64;
                    decoder.push((now - self.start).as_secs_f64(), self.log_buf[..n].to_vec());
                    if n < self.log_buf.len() {
                        break;
                    }
                }
                Err(e) => {
                    self.last_error = Some(e.to_string());
                    break;
                }
            }
        }
    }

    /// Answer queued one-off reads.
    fn serve_reads(&mut self, memory: &mut dyn MemoryAccess) {
        for (regions, reply) in std::mem::take(&mut self.reads) {
            let bytes = regions
                .into_iter()
                .map(|(address, len)| {
                    let mut buf = vec![0; len];
                    memory.read(address, &mut buf).map(|()| buf)
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string());
            let _ = reply.send(bytes);
        }
    }

    /// Send queued requests; `true` when any was attempted. They go to the
    /// firmware over RTT when it serves requests there, and are written into
    /// the table's cells otherwise.
    fn serve_requests(&mut self, memory: &mut dyn MemoryAccess) -> bool {
        if self.requests.is_empty() {
            return false;
        }
        let now = Instant::now();
        let mut framed = self
            .framed
            .as_mut()
            .and_then(|framed| framed.attach(memory, now).then_some(framed));
        for (request, reply) in std::mem::take(&mut self.requests) {
            let Some(tuner) = &mut self.tuner else {
                let _ = reply.send(Err("the open ELF declares no tuning table".into()));
                continue;
            };
            if let Some(framed) = framed.as_deref_mut() {
                let framed_request = match request {
                    TuneRequest::Set { id, value } => tuner
                        .encode(id, value)
                        .map(|(tag, bits)| FramedRequest::Write { id, tag, bits }),
                    TuneRequest::Discard => Ok(FramedRequest::Discard),
                    TuneRequest::Save => Ok(FramedRequest::Save),
                };
                match framed_request {
                    Ok(r) => framed.submit(memory, now, r, reply),
                    Err(e) => {
                        let _ = reply.send(Err(e));
                    }
                }
                continue;
            }
            if let Some(e) = tuner.verify(memory) {
                self.last_error = Some(e);
            }
            let result = match request {
                TuneRequest::Set { id, value } => tuner.request(memory, id, value),
                TuneRequest::Discard => tuner.discard(memory),
                TuneRequest::Save => Err(
                    "this firmware takes no requests over RTT, so it cannot save from a probe; connect over USB to save"
                        .into(),
                ),
            };
            let _ = reply.send(result);
        }
        true
    }

    fn report_tune(&mut self, memory: &mut dyn MemoryAccess) {
        let Some(tuner) = &mut self.tuner else {
            return;
        };
        if let Some(e) = tuner.verify(memory) {
            self.last_error = Some(e);
        }
        // Cells of another build hold unrelated memory, not values worth showing
        let values = if *tuner.check() == CatalogCheck::Matches {
            tuner.sample(memory)
        } else {
            Vec::new()
        };
        self.sink.event(SessionEvent::Tune {
            check: tuner.check().clone(),
            values,
        });
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

    fn report_stats(&mut self, memory: &mut dyn MemoryAccess) {
        let core = cortex_m::core_state(memory).unwrap_or(CoreState::Unknown);
        let log = self
            .rtt
            .as_ref()
            .map_or(StreamState::Absent, RttReader::state);
        let skipped = self.skipped_before + self.sampler.as_ref().map_or(0, Ticker::skipped);
        let stats = self.stats.snapshot(
            if self.sampler.is_some() {
                self.rate_hz
            } else {
                0.0
            },
            skipped,
            self.plan.regions().len(),
            self.plan.items().len(),
        );
        self.sink.event(SessionEvent::Stats {
            stats,
            core,
            log,
            last_error: self.last_error.take(),
        });
    }
}
