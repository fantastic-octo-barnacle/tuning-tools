//! The hardware owner: one thread per connection owns the carrier, samples the
//! watched set on absolute deadlines, pumps the log stream and reports status.
//! Everything else talks to it through [`SessionCommand`]s.

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::Serialize;
use studio_carriers::{CarrierError, CoreState, Link, StreamState};

use crate::frame::FrameBuilder;
use crate::log::{LogDecoder, LogLine};
use crate::plan::{ReadItem, ReadPlan};
use crate::schedule::Ticker;
use crate::stats::{LinkStats, StatsSnapshot};

pub const FRAME_PERIOD: Duration = Duration::from_millis(33);
pub const LOG_PERIOD: Duration = Duration::from_millis(10);
pub const STATS_PERIOD: Duration = Duration::from_millis(500);
pub const MAX_RATE_HZ: f64 = 2000.0;
/// Every read failing for this long means the link is gone.
pub const LINK_LOST_AFTER: Duration = Duration::from_secs(2);

pub enum SessionCommand {
    SetWatches(Vec<ReadItem>),
    SetRate(f64),
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
}

/// Where a session delivers its output. Called from the session threads.
pub trait SessionSink: Send + Sync + 'static {
    /// Deliver an encoded sample frame; `false` when it could not be delivered.
    fn frame(&self, bytes: Vec<u8>) -> bool;
    fn event(&self, event: SessionEvent);
}

pub struct SessionOptions {
    pub rate_hz: f64,
    /// ELF bytes for the defmt table; `None` disables log decoding
    pub elf: Option<Vec<u8>>,
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
                let link = match connect() {
                    Ok(link) => link,
                    Err(e) => {
                        sink.event(SessionEvent::Status {
                            state: LinkState::Failed,
                            message: Some(e.to_string()),
                        });
                        return;
                    }
                };
                Worker::new(link, options, sink).run(rx);
            })
            .expect("spawn session thread");
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

struct Worker {
    link: Box<dyn Link>,
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
    log_buf: Vec<u8>,
    last_error: Option<String>,
    failing_since: Option<Instant>,
}

impl Worker {
    fn new(link: Box<dyn Link>, options: SessionOptions, sink: Arc<dyn SessionSink>) -> Self {
        let start = Instant::now();
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
        Self {
            link,
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
            log_buf: vec![0; 4096],
            last_error: None,
            failing_since: None,
        }
    }

    fn run(mut self, rx: mpsc::Receiver<SessionCommand>) {
        self.sink.event(SessionEvent::Status {
            state: LinkState::Connected,
            message: None,
        });
        let now = Instant::now();
        let mut log_tick = Ticker::new(now, LOG_PERIOD);
        let mut frame_tick = Ticker::new(now, FRAME_PERIOD);
        let mut stats_tick = Ticker::new(now + STATS_PERIOD, STATS_PERIOD);

        let failure = loop {
            let mut next = log_tick
                .deadline()
                .min(frame_tick.deadline())
                .min(stats_tick.deadline());
            if let Some(s) = &self.sampler {
                next = next.min(s.deadline());
            }
            let wait = next.saturating_duration_since(Instant::now());
            match rx.recv_timeout(wait) {
                Ok(SessionCommand::Stop) | Err(RecvTimeoutError::Disconnected) => break None,
                Ok(SessionCommand::SetWatches(items)) => self.set_watches(items),
                Ok(SessionCommand::SetRate(hz)) => self.set_rate(hz),
                Err(RecvTimeoutError::Timeout) => {}
            }

            let now = Instant::now();
            if self.sampler.as_mut().is_some_and(|s| s.poll(now)) {
                if let Some(message) = self.sample(now) {
                    break Some(message);
                }
            }
            if log_tick.poll(now) {
                self.pump_log(now);
            }
            if frame_tick.poll(now) {
                self.flush_frame();
            }
            if stats_tick.poll(now) {
                self.report_stats();
            }
        };

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
    fn sample(&mut self, now: Instant) -> Option<String> {
        let started = Instant::now();
        let outcome = self.plan.sample(self.link.memory(), &mut self.row);
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

    fn pump_log(&mut self, now: Instant) {
        let Some(decoder) = &self.decoder else {
            return;
        };
        // Drain what is buffered, bounded so a chatty target cannot starve sampling
        for _ in 0..8 {
            match self.link.log().read(&mut self.log_buf) {
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

    fn flush_frame(&mut self) {
        if let Some(bytes) = self.frame.flush() {
            if self.sink.frame(bytes) {
                self.stats.frames_sent += 1;
            } else {
                self.stats.frames_dropped += 1;
            }
        }
    }

    fn report_stats(&mut self) {
        let core = self.link.core_state().unwrap_or(CoreState::Unknown);
        let log = if self.decoder.is_some() {
            self.link.log().state()
        } else {
            StreamState::Absent
        };
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
