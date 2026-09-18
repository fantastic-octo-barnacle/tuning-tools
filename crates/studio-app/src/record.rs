//! Recording a session to MCAP, and reading recordings back as CSV.
//!
//! The recorder subscribes to the [`Tap`], so it sees every tick whatever the UI
//! drops, and writes on a thread of its own. Its queue is bounded: when the disk
//! cannot keep up, whole batches are dropped and counted, and sampling goes on.
//!
//! Layout (every channel JSON, with a `jsonschema` schema):
//!
//! - `/watches`: one message per tick, `{"t": <session s>, "<name>": value, ...}`;
//!   a value that failed to read (NaN) or is not finite is left out.
//! - `/watches/meta`: the columns, `{"watches": [{id, name, path, type, unit}]}`,
//!   written before the first tick and again whenever the watched set changes.
//! - `/log`: the firmware's defmt log as `foxglove.Log`.
//! - `/tune`: tuning values when they change, and the requests the app sent.
//! - `/session`: link status, stats snapshots, the tuning table check.
//! - Metadata `session` (at the start) and `recording_end` (on a clean stop).
//!
//! Message times are the session's wall-clock start plus session seconds. Chunks
//! are closed and flushed every second, so a killed process leaves a file that
//! reads up to its last chunk.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Local};
use mcap::records::{MessageHeader, Metadata};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use studio_core::log::LogLine;
use studio_core::session::LinkState;
use studio_core::tap::{Subscription, TapEvent, TapSnapshot};
use studio_core::tune::{CatalogCheck, TuneValue};
use studio_core::{SampleBatch, SessionEvent, Tap, WatchSet};

use crate::{AppEvent, AppEventSink, StudioApp, APP_VERSION};

/// Tap messages the recorder may fall behind by: about two minutes of batches
const QUEUE: usize = 4096;
/// How often a chunk is closed and flushed, and progress reported
const FLUSH_EVERY: Duration = Duration::from_secs(1);
/// Uncompressed size that closes a chunk early
const CHUNK_SIZE: u64 = 1 << 20;

pub const WATCHES: &str = "/watches";
pub const WATCHES_META: &str = "/watches/meta";
pub const LOG: &str = "/log";
pub const TUNE: &str = "/tune";
pub const SESSION: &str = "/session";

/// A recording in progress, or how one ended.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecordingState {
    pub active: bool,
    pub path: String,
    /// Seconds since the recording started
    pub elapsed: f64,
    /// Ticks written
    pub ticks: u64,
    /// Bytes on disk
    pub bytes: u64,
    /// Sample batches lost because writing fell behind
    pub dropped: u64,
    /// Why the recording stopped early, if it did
    pub error: Option<String>,
}

pub(crate) struct Recorder {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<RecordingState>>,
    progress: Arc<Mutex<RecordingState>>,
}

impl Recorder {
    fn start(path: PathBuf, tap: &Arc<Tap>, events: Option<AppEventSink>) -> Result<Self, String> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
        }
        // Subscribe first so nothing published while the file opens is missed
        let (subscription, snapshot) = tap.subscribe(QUEUE);
        let file =
            File::create(&path).map_err(|e| format!("could not create {}: {e}", path.display()))?;
        let out = Out::create(file, &snapshot).map_err(|e| format!("{}: {e}", path.display()))?;
        let progress = Arc::new(Mutex::new(RecordingState {
            active: true,
            path: path.display().to_string(),
            elapsed: 0.0,
            ticks: 0,
            bytes: 0,
            dropped: 0,
            error: None,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let worker = Worker {
            out,
            path,
            subscription,
            watches: None,
            keys: Vec::new(),
            tune: HashMap::new(),
            names: snapshot
                .info
                .as_ref()
                .map(|i| i.tunables.iter().map(|t| (t.id, t.name.clone())).collect())
                .unwrap_or_default(),
            check: snapshot.check.clone(),
            ticks: 0,
            started: Instant::now(),
            progress: progress.clone(),
            events,
            json: Vec::with_capacity(256),
        };
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("recorder".into())
            .spawn(move || worker.run(&flag))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            stop,
            thread: Some(thread),
            progress,
        })
    }

    pub(crate) fn state(&self) -> RecordingState {
        self.progress
            .lock()
            .expect("recording state poisoned")
            .clone()
    }

    fn is_active(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Write what is queued, close the file properly and report how it went.
    fn finish(mut self) -> RecordingState {
        self.stop.store(true, Ordering::Relaxed);
        match self.thread.take().map(JoinHandle::join) {
            Some(Ok(state)) => state,
            _ => {
                let mut state = self.state();
                state.active = false;
                state
                    .error
                    .get_or_insert("the recorder stopped unexpectedly".into());
                state
            }
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

type McapWriter = mcap::Writer<BufWriter<File>>;

/// The file and its channels.
struct Out {
    /// `None` once closed
    writer: Option<McapWriter>,
    channels: [u16; 5],
    sequence: [u32; 5],
    /// Wall-clock nanoseconds of session time zero
    epoch_ns: u64,
}

#[derive(Clone, Copy)]
enum Channel {
    Watches = 0,
    WatchesMeta = 1,
    Log = 2,
    Tune = 3,
    Session = 4,
}

fn unix_ns(at: SystemTime) -> u64 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

fn rfc3339(at: SystemTime) -> String {
    DateTime::<Local>::from(at).to_rfc3339()
}

const OBJECT_SCHEMA: &str = r#"{"type":"object"}"#;

fn watches_schema() -> String {
    json!({
        "title": "tuning_tools.Watches",
        "description": "One tick of the watched values; a key is absent when its read failed",
        "type": "object",
        "properties": { "t": { "type": "number", "description": "Seconds since the session started" } },
        "additionalProperties": { "type": "number" },
    })
    .to_string()
}

fn meta_schema() -> String {
    json!({
        "title": "tuning_tools.WatchesMeta",
        "type": "object",
        "properties": {
            "t": { "type": "number" },
            "watches": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "integer" },
                        "name": { "type": "string" },
                        "path": { "type": "string" },
                        "type": { "type": "string" },
                        "unit": { "type": ["string", "null"] },
                    },
                },
            },
        },
    })
    .to_string()
}

/// foxglove.Log, so Foxglove's Log panel shows the firmware's log
fn log_schema() -> String {
    json!({
        "title": "foxglove.Log",
        "description": "A log message",
        "type": "object",
        "properties": {
            "timestamp": {
                "type": "object",
                "title": "time",
                "properties": {
                    "sec": { "type": "integer", "minimum": 0 },
                    "nsec": { "type": "integer", "minimum": 0, "maximum": 999_999_999 },
                },
            },
            "level": {
                "title": "foxglove.LogLevel",
                "oneOf": [
                    { "title": "UNKNOWN", "const": 0 },
                    { "title": "DEBUG", "const": 1 },
                    { "title": "INFO", "const": 2 },
                    { "title": "WARNING", "const": 3 },
                    { "title": "ERROR", "const": 4 },
                    { "title": "FATAL", "const": 5 },
                ],
            },
            "message": { "type": "string" },
            "name": { "type": "string" },
            "file": { "type": "string" },
            "line": { "type": "integer", "minimum": 0 },
        },
    })
    .to_string()
}

impl Out {
    fn create(file: File, snapshot: &TapSnapshot) -> Result<Self, mcap::McapError> {
        let options = mcap::WriteOptions::new()
            .compression(Some(mcap::Compression::Zstd))
            .compression_threads(0)
            .chunk_size(Some(CHUNK_SIZE))
            .library(format!("tuning-tools {APP_VERSION}"));
        let mut writer = options.create(BufWriter::new(file))?;
        let none = BTreeMap::new();
        let mut channel = |schema: &str, data: &str, topic: &str| -> Result<u16, mcap::McapError> {
            let schema = writer.add_schema(schema, "jsonschema", data.as_bytes())?;
            writer.add_channel(schema, topic, "json", &none)
        };
        let channels = [
            channel("tuning_tools.Watches", &watches_schema(), WATCHES)?,
            channel("tuning_tools.WatchesMeta", &meta_schema(), WATCHES_META)?,
            channel("foxglove.Log", &log_schema(), LOG)?,
            channel("tuning_tools.Tune", OBJECT_SCHEMA, TUNE)?,
            channel("tuning_tools.Session", OBJECT_SCHEMA, SESSION)?,
        ];
        let now = SystemTime::now();
        let clock = snapshot.clock.unwrap_or(now);
        let mut metadata = BTreeMap::new();
        let mut put = |key: &str, value: String| {
            metadata.insert(key.to_string(), value);
        };
        put("app_version", APP_VERSION.into());
        put("recording_start", rfc3339(now));
        put("session_start", rfc3339(clock));
        put("session_start_unix_ns", unix_ns(clock).to_string());
        put("link", format!("{:?}", snapshot.link).to_lowercase());
        if let Some(info) = &snapshot.info {
            put("carrier", info.carrier.clone());
            put("rate_hz", info.rate_hz.to_string());
            for (key, value) in [
                ("elf", &info.elf),
                ("elf_path", &info.elf_path),
                ("build_id", &info.build_id),
                ("chip", &info.chip),
                ("port", &info.port),
            ] {
                if let Some(value) = value {
                    put(key, value.clone());
                }
            }
        }
        put(
            "elf_match",
            match &snapshot.check {
                None => "unknown".into(),
                Some(CatalogCheck::Checking) => "checking".into(),
                Some(CatalogCheck::Matches) => "matches".into(),
                Some(CatalogCheck::Differs { message }) => format!("differs: {message}"),
            },
        );
        writer.write_metadata(&Metadata {
            name: "session".into(),
            metadata,
        })?;
        let mut out = Self {
            writer: Some(writer),
            channels,
            sequence: [0; 5],
            epoch_ns: unix_ns(clock),
        };
        if let Some(info) = &snapshot.info {
            let mut info = serde_json::to_value(&**info).unwrap_or(Value::Null);
            info["kind"] = "info".into();
            out.write_value(Channel::Session, out.now_ns(), &info)?;
        }
        Ok(out)
    }

    fn writer(&mut self) -> &mut McapWriter {
        self.writer.as_mut().expect("the recording is still open")
    }

    fn at(&self, t: f64) -> u64 {
        self.epoch_ns
            .saturating_add_signed((t * 1e9).round() as i64)
    }

    fn now_ns(&self) -> u64 {
        unix_ns(SystemTime::now())
    }

    /// Session seconds of a wall-clock time
    fn session_t(&self, ns: u64) -> f64 {
        (ns as i128 - self.epoch_ns as i128) as f64 / 1e9
    }

    fn write(&mut self, channel: Channel, time: u64, data: &[u8]) -> Result<(), mcap::McapError> {
        let i = channel as usize;
        self.sequence[i] = self.sequence[i].wrapping_add(1);
        let header = MessageHeader {
            channel_id: self.channels[i],
            sequence: self.sequence[i],
            log_time: time,
            publish_time: time,
        };
        self.writer().write_to_known_channel(&header, data)
    }

    fn write_value(
        &mut self,
        channel: Channel,
        time: u64,
        value: &Value,
    ) -> Result<(), mcap::McapError> {
        let bytes = serde_json::to_vec(value).expect("JSON values serialise");
        self.write(channel, time, &bytes)
    }
}

/// Why the writing loop ended
enum End {
    Stopped,
    SessionEnded,
    Failed(String),
}

struct Worker {
    out: Out,
    path: PathBuf,
    subscription: Subscription,
    /// The columns of the ticks written last
    watches: Option<Arc<WatchSet>>,
    /// `,"<name>":` for each column, JSON-escaped
    keys: Vec<Vec<u8>>,
    /// Last tuning values written, by id
    tune: HashMap<u32, TuneValue>,
    /// Tunable names by id
    names: HashMap<u32, String>,
    check: Option<CatalogCheck>,
    ticks: u64,
    started: Instant,
    progress: Arc<Mutex<RecordingState>>,
    events: Option<AppEventSink>,
    json: Vec<u8>,
}

impl Worker {
    fn run(mut self, stop: &AtomicBool) -> RecordingState {
        let mut next_flush = Instant::now() + FLUSH_EVERY;
        let end = loop {
            if stop.load(Ordering::Relaxed) {
                // Write what was queued before the stop, then close
                let mut end = End::Stopped;
                while let Some(event) = self.subscription.try_recv() {
                    match self.handle(event) {
                        Ok(true) => {}
                        Ok(false) => break,
                        Err(e) => {
                            end = End::Failed(e.to_string());
                            break;
                        }
                    }
                }
                break end;
            }
            match self.subscription.recv_timeout(Duration::from_millis(50)) {
                Ok(event) => match self.handle(event) {
                    Ok(true) => {}
                    Ok(false) => break End::SessionEnded,
                    Err(e) => break End::Failed(e.to_string()),
                },
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break End::Stopped,
            }
            if Instant::now() >= next_flush {
                next_flush += FLUSH_EVERY;
                if let Err(e) = self.out.writer().flush() {
                    break End::Failed(e.to_string());
                }
                self.report(true, None);
            }
        };

        let closed = match end {
            End::Failed(e) => Err(format!("recording stopped: {e}")),
            End::Stopped | End::SessionEnded => self.close(),
        };
        let mut state = self.report(false, closed.err());
        state.bytes = std::fs::metadata(&self.path).map_or(state.bytes, |m| m.len());
        *self.progress.lock().expect("recording state poisoned") = state.clone();
        if let Some(events) = &self.events {
            events(AppEvent::Recording(state.clone()));
        }
        state
    }

    /// Update and publish progress; the final report is sent by `run`.
    fn report(&mut self, active: bool, error: Option<String>) -> RecordingState {
        let state = RecordingState {
            active,
            path: self.path.display().to_string(),
            elapsed: self.started.elapsed().as_secs_f64(),
            ticks: self.ticks,
            bytes: std::fs::metadata(&self.path).map_or(0, |m| m.len()),
            dropped: self.subscription.drops().batches(),
            error,
        };
        *self.progress.lock().expect("recording state poisoned") = state.clone();
        if active {
            if let Some(events) = &self.events {
                events(AppEvent::Recording(state.clone()));
            }
        }
        state
    }

    /// Write the summary and footer, and get the file onto the disk.
    fn close(&mut self) -> Result<(), String> {
        let drops = self.subscription.drops();
        let mut metadata = BTreeMap::new();
        metadata.insert("ticks".into(), self.ticks.to_string());
        metadata.insert("dropped_batches".into(), drops.batches().to_string());
        metadata.insert("dropped_ticks".into(), drops.ticks().to_string());
        metadata.insert("dropped_events".into(), drops.events().to_string());
        metadata.insert(
            "duration_s".into(),
            self.started.elapsed().as_secs_f64().to_string(),
        );
        metadata.insert("recording_end".into(), rfc3339(SystemTime::now()));
        let mut writer = self.out.writer.take().expect("closed once");
        writer
            .write_metadata(&Metadata {
                name: "recording_end".into(),
                metadata,
            })
            .and_then(|()| writer.finish())
            .map_err(|e| e.to_string())?;
        writer
            .into_inner()
            .into_inner()
            .map_err(|e| e.error().to_string())?
            .sync_all()
            .map_err(|e| e.to_string())
    }

    /// Write one tap message; `Ok(false)` when the session has ended.
    fn handle(&mut self, event: TapEvent) -> Result<bool, mcap::McapError> {
        match event {
            TapEvent::Samples { batch, watches } => self.samples(&batch, &watches)?,
            TapEvent::Watches(set) => {
                let at = self.out.now_ns();
                self.columns(&set, self.out.session_t(at), at)?;
            }
            TapEvent::Info(info) => {
                self.names = info
                    .tunables
                    .iter()
                    .map(|t| (t.id, t.name.clone()))
                    .collect();
                let mut value = serde_json::to_value(&*info).unwrap_or(Value::Null);
                value["kind"] = "info".into();
                self.out
                    .write_value(Channel::Session, self.out.now_ns(), &value)?;
            }
            TapEvent::Clock(at) => self.out.epoch_ns = unix_ns(at),
            TapEvent::TuneRequest(request) => {
                let name = request.id.and_then(|id| self.names.get(&id).cloned());
                let value = json!({
                    "kind": "request",
                    "action": request.action,
                    "id": request.id,
                    "name": name,
                    "value": request.value,
                    "ok": request.error.is_none(),
                    "error": request.error,
                });
                self.out
                    .write_value(Channel::Tune, self.out.now_ns(), &value)?;
            }
            TapEvent::Event(event) => return self.session_event(&event),
        }
        Ok(true)
    }

    fn session_event(&mut self, event: &SessionEvent) -> Result<bool, mcap::McapError> {
        let now = self.out.now_ns();
        match event {
            SessionEvent::Status { state, message } => {
                let value = json!({ "kind": "status", "state": state, "message": message });
                self.out.write_value(Channel::Session, now, &value)?;
                return Ok(!matches!(
                    state,
                    LinkState::Disconnected | LinkState::Failed
                ));
            }
            SessionEvent::Stats { .. } => {
                let mut value = serde_json::to_value(event).unwrap_or(Value::Null);
                value["kind"] = "stats".into();
                if let Some(map) = value.as_object_mut() {
                    map.remove("type");
                }
                self.out.write_value(Channel::Session, now, &value)?;
            }
            SessionEvent::Log { lines } => {
                for line in lines {
                    let at = self.out.at(line.host_time);
                    let bytes = serde_json::to_vec(&foxglove_log(line, at)).expect("JSON");
                    self.out.write(Channel::Log, at, &bytes)?;
                }
            }
            SessionEvent::Tune { check, values } => {
                if self.check.as_ref() != Some(check) {
                    self.check = Some(check.clone());
                    let value = json!({ "kind": "check", "check": check });
                    self.out.write_value(Channel::Session, now, &value)?;
                }
                let changed = values.len() != self.tune.len()
                    || values.iter().any(|v| self.tune.get(&v.id) != Some(v));
                if changed && !values.is_empty() {
                    self.tune = values.iter().map(|v| (v.id, v.clone())).collect();
                    let values: Vec<Value> = values
                        .iter()
                        .map(|v| {
                            json!({
                                "id": v.id,
                                "name": self.names.get(&v.id),
                                "requested": v.requested,
                                "applied": v.applied,
                            })
                        })
                        .collect();
                    let value = json!({ "kind": "values", "values": values });
                    self.out.write_value(Channel::Tune, now, &value)?;
                }
            }
            SessionEvent::Catalog { catalog } => {
                self.names = catalog
                    .entries
                    .iter()
                    .map(|e| (e.id, e.name.clone()))
                    .collect();
            }
        }
        Ok(true)
    }

    /// Write the column set when it differs from the last one written.
    fn columns(&mut self, set: &Arc<WatchSet>, t: f64, at: u64) -> Result<(), mcap::McapError> {
        if self.watches.as_ref().is_some_and(|w| **w == **set) {
            return Ok(());
        }
        self.keys = set
            .watches()
            .iter()
            .map(|w| {
                let mut key = b",".to_vec();
                serde_json::to_writer(&mut key, &w.name).expect("JSON string");
                key.push(b':');
                key
            })
            .collect();
        self.watches = Some(set.clone());
        let value = json!({ "t": t, "watches": &**set });
        self.out.write_value(Channel::WatchesMeta, at, &value)
    }

    fn samples(&mut self, batch: &SampleBatch, set: &Arc<WatchSet>) -> Result<(), mcap::McapError> {
        let Some(&first) = batch.times.first() else {
            return Ok(());
        };
        self.columns(set, first, self.out.at(first))?;
        let mut number = ryu::Buffer::new();
        for (row, &t) in batch.times.iter().enumerate() {
            self.json.clear();
            self.json.extend_from_slice(b"{\"t\":");
            self.json.extend_from_slice(number.format(t).as_bytes());
            for (key, &value) in self.keys.iter().zip(batch.row(row)) {
                if value.is_finite() {
                    self.json.extend_from_slice(key);
                    self.json
                        .extend_from_slice(number.format_finite(value).as_bytes());
                }
            }
            self.json.push(b'}');
            let at = self.out.at(t);
            let json = std::mem::take(&mut self.json);
            let written = self.out.write(Channel::Watches, at, &json);
            self.json = json;
            written?;
        }
        self.ticks += batch.ticks() as u64;
        Ok(())
    }
}

fn foxglove_log(line: &LogLine, at: u64) -> Value {
    let level = match line.level.as_deref() {
        Some("trace" | "debug") => 1,
        Some("info") => 2,
        Some("warn") => 3,
        Some("error") => 4,
        _ => 0,
    };
    let (file, number) = line
        .location
        .as_deref()
        .and_then(|l| l.rsplit_once(':'))
        .map_or((None, None), |(f, n)| (Some(f), n.parse::<u32>().ok()));
    let message = match &line.timestamp {
        Some(ts) => format!("[{ts}] {}", line.message),
        None => line.message.clone(),
    };
    json!({
        "timestamp": { "sec": at / 1_000_000_000, "nsec": at % 1_000_000_000 },
        "level": level,
        "message": message,
        "name": line.module.as_deref().unwrap_or("firmware"),
        "file": file.unwrap_or(""),
        "line": number.unwrap_or(0),
    })
}

/// `<elf>-<YYYYMMDD-HHMMSS>.mcap` in `dir`, not taken yet
fn default_path(dir: &Path, elf: Option<&str>) -> PathBuf {
    let stem = elf
        .map(|e| {
            Path::new(e)
                .file_stem()
                .map_or(e.into(), |s| s.to_string_lossy())
        })
        .filter(|s| !s.is_empty())
        .map_or_else(|| "recording".to_string(), |s| s.into_owned());
    let stamp = Local::now().format("%Y%m%d-%H%M%S");
    let mut path = dir.join(format!("{stem}-{stamp}.mcap"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{stem}-{stamp}-{n}.mcap"));
        n += 1;
    }
    path
}

impl StudioApp {
    /// Start recording the connected session to `path`, or to a new file in
    /// `dir` (else the host's recordings directory) when there is none.
    pub fn start_recording(
        &self,
        path: Option<PathBuf>,
        dir: Option<PathBuf>,
    ) -> Result<RecordingState, String> {
        let mut slot = self.recorder.lock().expect("recorder poisoned");
        if slot.as_ref().is_some_and(Recorder::is_active) {
            return Err("already recording; stop that recording first".into());
        }
        if self.tap.link_state() != LinkState::Connected {
            return Err("connect to the target before recording".into());
        }
        let path = match path {
            Some(path) => path,
            None => {
                let dir = dir
                    .or_else(|| self.recordings_dir.lock().expect("dir poisoned").clone())
                    .unwrap_or_else(|| std::env::temp_dir().join("tuning-studio-recordings"));
                let info = self.tap.snapshot().info;
                default_path(&dir, info.as_ref().and_then(|i| i.elf.as_deref()))
            }
        };
        let recorder = Recorder::start(path, &self.tap, self.event_sink())?;
        let state = recorder.state();
        // A finished recorder from before is simply replaced
        *slot = Some(recorder);
        if let Some(events) = self.event_sink() {
            events(AppEvent::Recording(state.clone()));
        }
        Ok(state)
    }

    /// Stop recording and close the file; how the recording went.
    pub fn stop_recording(&self) -> Result<RecordingState, String> {
        let recorder = self.recorder.lock().expect("recorder poisoned").take();
        recorder
            .map(Recorder::finish)
            .ok_or_else(|| "not recording".to_string())
    }

    /// End a recording along with its session; the recorder reports how it ended.
    pub(crate) fn finish_recording(&self) {
        let recorder = self.recorder.lock().expect("recorder poisoned").take();
        if let Some(recorder) = recorder {
            recorder.finish();
        }
    }
}

/// What [`export_csv`] wrote.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CsvExport {
    pub path: String,
    pub rows: u64,
    /// Value columns, besides `time`
    pub columns: usize,
    /// The recording ended without its footer (the app was killed); read up to its last chunk
    pub truncated: bool,
}

#[derive(Deserialize)]
struct MetaMessage {
    watches: Vec<MetaColumn>,
}

#[derive(Deserialize)]
struct MetaColumn {
    name: String,
    unit: Option<String>,
}

/// The messages of a recording, as far as it reads; `true` when it ended early
fn read_messages(bytes: &[u8], mut each: impl FnMut(&str, &[u8])) -> Result<bool, String> {
    let stream =
        mcap::MessageStream::new_with_options(bytes, mcap::read::Options::IgnoreEndMagic.into())
            .map_err(|e| format!("not an MCAP recording: {e}"))?;
    for message in stream {
        match message {
            Ok(m) => each(&m.channel.topic, &m.data),
            Err(_) => return Ok(true),
        }
    }
    Ok(mcap::read::footer(bytes).is_err())
}

fn csv_field(out: &mut String, text: &str) {
    if text.contains([',', '"', '\n', '\r']) {
        out.push('"');
        out.push_str(&text.replace('"', "\"\""));
        out.push('"');
    } else {
        out.push_str(text);
    }
}

fn csv_number(out: &mut String, v: f64) {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        out.push_str(&(v as i64).to_string());
    } else {
        out.push_str(ryu::Buffer::new().format_finite(v));
    }
}

/// Write a recording's `/watches` as a wide CSV: `time`, then one column per
/// value over the whole file, empty where the value was not watched or failed
/// to read. `csv` defaults to the recording's path with a `.csv` extension.
pub fn export_csv(mcap: &Path, csv: Option<&Path>) -> Result<CsvExport, String> {
    let bytes =
        std::fs::read(mcap).map_err(|e| format!("could not read {}: {e}", mcap.display()))?;
    // First pass: the union of columns, in order of appearance
    let mut columns: Vec<(String, Option<String>)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut add =
        |name: &str, unit: Option<String>, columns: &mut Vec<(String, Option<String>)>| {
            match index.get(name) {
                // The latest unit given wins
                Some(&i) => {
                    if unit.is_some() {
                        columns[i].1 = unit;
                    }
                }
                None => {
                    index.insert(name.to_string(), columns.len());
                    columns.push((name.to_string(), unit));
                }
            }
        };
    let truncated = read_messages(&bytes, |topic, data| match topic {
        WATCHES_META => {
            if let Ok(meta) = serde_json::from_slice::<MetaMessage>(data) {
                for c in meta.watches {
                    add(&c.name, c.unit, &mut columns);
                }
            }
        }
        WATCHES => {
            if let Ok(row) = serde_json::from_slice::<HashMap<&str, serde::de::IgnoredAny>>(data) {
                for key in row.keys().filter(|k| **k != "t") {
                    add(key, None, &mut columns);
                }
            }
        }
        _ => {}
    })?;
    let index: HashMap<&str, usize> = columns
        .iter()
        .enumerate()
        .map(|(i, (name, _))| (name.as_str(), i))
        .collect();

    let path = csv.map_or_else(|| mcap.with_extension("csv"), Path::to_path_buf);
    let file =
        File::create(&path).map_err(|e| format!("could not create {}: {e}", path.display()))?;
    let mut out = BufWriter::new(file);
    let mut line = String::from("time");
    for (name, unit) in &columns {
        line.push(',');
        match unit {
            Some(unit) => csv_field(&mut line, &format!("{name} [{unit}]")),
            None => csv_field(&mut line, name),
        }
    }
    line.push('\n');
    let mut error = out.write_all(line.as_bytes()).err();
    let mut rows = 0u64;
    let mut cells: Vec<Option<f64>> = vec![None; columns.len()];
    read_messages(&bytes, |topic, data| {
        if topic != WATCHES || error.is_some() {
            return;
        }
        let Ok(row) = serde_json::from_slice::<HashMap<&str, Value>>(data) else {
            return;
        };
        let Some(t) = row.get("t").and_then(Value::as_f64) else {
            return;
        };
        cells.iter_mut().for_each(|c| *c = None);
        for (key, value) in &row {
            if let (Some(&i), Some(v)) = (index.get(key), value.as_f64()) {
                cells[i] = Some(v);
            }
        }
        line.clear();
        line.push_str(ryu::Buffer::new().format(t));
        for cell in &cells {
            line.push(',');
            if let Some(v) = cell {
                csv_number(&mut line, *v);
            }
        }
        line.push('\n');
        match out.write_all(line.as_bytes()) {
            Ok(()) => rows += 1,
            Err(e) => error = Some(e),
        }
    })?;
    if let Some(e) = error.or_else(|| out.flush().err()) {
        return Err(format!("could not write {}: {e}", path.display()));
    }
    Ok(CsvExport {
        path: path.display().to_string(),
        rows,
        columns: columns.len(),
        truncated,
    })
}
