//! The tap: every flushed sample batch and session event, handed to
//! subscribers (a recorder, the TCP stream) at the full sampling rate.
//!
//! It sits in front of the UI sink, so a UI that refuses frames (a full stdout
//! queue, a lagging webview) loses nothing here. Publishing never blocks: each
//! subscriber has a bounded queue, and what does not fit is dropped and
//! counted on that subscription.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde::Serialize;

use crate::frame::SampleBatch;
use crate::session::{LinkState, SessionEvent, SessionSink};
use crate::tune::CatalogCheck;

/// What a sampled value is, for files and scripts that do not know the ELF.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WatchMeta {
    pub id: u32,
    /// Unique within its set; the key a value is written under
    pub name: String,
    /// Symbol path, or the tuning value's name
    pub path: String,
    #[serde(rename = "type")]
    pub type_name: String,
    pub unit: Option<String>,
}

/// The values one batch holds, in column order.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
#[serde(transparent)]
pub struct WatchSet(Vec<WatchMeta>);

impl WatchSet {
    /// Names are made unique (and never `t`, the time key) by appending `#id`.
    pub fn new(mut watches: Vec<WatchMeta>) -> Self {
        let mut taken: std::collections::HashSet<String> = ["t".to_string()].into();
        for w in &mut watches {
            if w.name.is_empty() {
                w.name = format!("id{}", w.id);
            }
            if !taken.insert(w.name.clone()) {
                w.name = format!("{}#{}", w.name, w.id);
                taken.insert(w.name.clone());
            }
        }
        Self(watches)
    }

    pub fn watches(&self) -> &[WatchMeta] {
        &self.0
    }

    pub fn ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.0.iter().map(|w| w.id)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// A tunable's name, for tune messages keyed by id.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TunableName {
    pub id: u32,
    pub name: String,
    pub unit: Option<String>,
}

/// What the session is connected to; set by the app at connect.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    /// The ELF's file name
    pub elf: Option<String>,
    pub elf_path: Option<String>,
    /// GNU build id, hex, when the ELF has one
    pub build_id: Option<String>,
    pub chip: Option<String>,
    /// `probe` or `serial`
    pub carrier: String,
    pub port: Option<String>,
    pub rate_hz: f64,
    pub app_version: String,
    pub tunables: Vec<TunableName>,
}

/// A tuning request the app sent, with how it went.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TuneRequest {
    /// `set`, `discard` or `save`
    pub action: &'static str,
    pub id: Option<u32>,
    pub value: Option<f64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum TapEvent {
    /// One flushed batch, with the metadata of its columns
    Samples {
        batch: Arc<SampleBatch>,
        watches: Arc<WatchSet>,
    },
    /// The watched set changed (or its names and units did)
    Watches(Arc<WatchSet>),
    /// A connect: what the new session talks to
    Info(Arc<SessionInfo>),
    /// When the new session's time zero is, on the wall clock
    Clock(SystemTime),
    /// Status, stats, log lines, tuning values, a link's catalog
    Event(Arc<SessionEvent>),
    TuneRequest(Arc<TuneRequest>),
}

/// What a new subscriber needs to make sense of what follows.
#[derive(Debug, Clone)]
pub struct TapSnapshot {
    pub watches: Arc<WatchSet>,
    pub info: Option<Arc<SessionInfo>>,
    pub link: LinkState,
    pub clock: Option<SystemTime>,
    pub check: Option<CatalogCheck>,
}

/// Messages a subscription lost to a full queue.
#[derive(Debug, Default)]
pub struct Drops {
    pub batches: AtomicU64,
    pub ticks: AtomicU64,
    pub events: AtomicU64,
}

impl Drops {
    pub fn batches(&self) -> u64 {
        self.batches.load(Ordering::Relaxed)
    }
    pub fn ticks(&self) -> u64 {
        self.ticks.load(Ordering::Relaxed)
    }
    pub fn events(&self) -> u64 {
        self.events.load(Ordering::Relaxed)
    }
}

struct Subscriber {
    key: u64,
    tx: SyncSender<TapEvent>,
    drops: Arc<Drops>,
}

struct Inner {
    subscribers: Vec<Subscriber>,
    next_key: u64,
    /// Metadata for every id seen, latest first; batches look their ids up here
    meta: HashMap<u32, WatchMeta>,
    /// The set last given to [`Tap::set_watches`]
    watches: Arc<WatchSet>,
    /// The set built for the last batch, reused while its ids stay the same
    batch_set: Option<(Vec<u32>, Arc<WatchSet>)>,
    info: Option<Arc<SessionInfo>>,
    link: LinkState,
    clock: Option<SystemTime>,
    check: Option<CatalogCheck>,
}

pub struct Tap {
    inner: Mutex<Inner>,
}

impl Default for Tap {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner {
                subscribers: Vec::new(),
                next_key: 0,
                meta: HashMap::new(),
                watches: Arc::default(),
                batch_set: None,
                info: None,
                link: LinkState::Disconnected,
                clock: None,
                check: None,
            }),
        }
    }
}

impl Tap {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("tap poisoned")
    }

    /// Receive everything published from now on, up to `capacity` queued messages.
    pub fn subscribe(self: &Arc<Self>, capacity: usize) -> (Subscription, TapSnapshot) {
        let mut inner = self.lock();
        let (tx, rx) = mpsc::sync_channel(capacity.max(1));
        let drops = Arc::new(Drops::default());
        let key = inner.next_key;
        inner.next_key += 1;
        inner.subscribers.push(Subscriber {
            key,
            tx,
            drops: drops.clone(),
        });
        let snapshot = TapSnapshot {
            watches: inner.watches.clone(),
            info: inner.info.clone(),
            link: inner.link,
            clock: inner.clock,
            check: inner.check.clone(),
        };
        let subscription = Subscription {
            rx,
            drops,
            key,
            tap: Arc::downgrade(self),
        };
        (subscription, snapshot)
    }

    pub fn snapshot(&self) -> TapSnapshot {
        let inner = self.lock();
        TapSnapshot {
            watches: inner.watches.clone(),
            info: inner.info.clone(),
            link: inner.link,
            clock: inner.clock,
            check: inner.check.clone(),
        }
    }

    pub fn link_state(&self) -> LinkState {
        self.lock().link
    }

    /// The watched set with its metadata, as the UI lists it.
    pub fn set_watches(&self, watches: Vec<WatchMeta>) {
        let mut inner = self.lock();
        for w in &watches {
            inner.meta.insert(w.id, w.clone());
        }
        let set = Arc::new(WatchSet::new(watches));
        if *set == *inner.watches {
            return;
        }
        let same_ids = set.ids().eq(inner.watches.ids());
        inner.watches = set.clone();
        inner.batch_set = None;
        // While sampling, a new set of ids reaches subscribers with the first
        // batch that has them: announcing it now would put it ahead of the
        // batch still being flushed for the old set
        if inner.link != LinkState::Connected || same_ids {
            publish(&mut inner, TapEvent::Watches(set));
        }
    }

    /// A new session is starting with `info`.
    pub fn connecting(&self, info: SessionInfo) {
        let mut inner = self.lock();
        let info = Arc::new(info);
        inner.info = Some(info.clone());
        inner.clock = None;
        inner.check = None;
        publish(&mut inner, TapEvent::Info(info));
    }

    /// The running session's sample rate changed.
    pub fn set_rate(&self, hz: f64) {
        let mut inner = self.lock();
        let Some(info) = &inner.info else {
            return;
        };
        let mut info = (**info).clone();
        info.rate_hz = hz;
        let info = Arc::new(info);
        inner.info = Some(info.clone());
        publish(&mut inner, TapEvent::Info(info));
    }

    pub fn tune_request(&self, request: TuneRequest) {
        let mut inner = self.lock();
        publish(&mut inner, TapEvent::TuneRequest(Arc::new(request)));
    }

    fn clock(&self, at: SystemTime) {
        let mut inner = self.lock();
        inner.clock = Some(at);
        publish(&mut inner, TapEvent::Clock(at));
    }

    fn samples(&self, batch: Arc<SampleBatch>) {
        let mut inner = self.lock();
        let watches = match &inner.batch_set {
            Some((ids, set)) if *ids == batch.ids => set.clone(),
            _ => {
                // Columns follow the session's ids; names come from the UI's list
                let set = if inner.watches.ids().eq(batch.ids.iter().copied()) {
                    inner.watches.clone()
                } else {
                    let metas = batch
                        .ids
                        .iter()
                        .map(|id| {
                            inner.meta.get(id).cloned().unwrap_or_else(|| WatchMeta {
                                id: *id,
                                name: format!("id{id}"),
                                path: String::new(),
                                type_name: String::new(),
                                unit: None,
                            })
                        })
                        .collect();
                    Arc::new(WatchSet::new(metas))
                };
                inner.batch_set = Some((batch.ids.clone(), set.clone()));
                set
            }
        };
        publish(&mut inner, TapEvent::Samples { batch, watches });
    }

    fn event(&self, event: &SessionEvent) {
        let mut inner = self.lock();
        match event {
            SessionEvent::Status { state, .. } => inner.link = *state,
            SessionEvent::Tune { check, .. } => inner.check = Some(check.clone()),
            _ => {}
        }
        publish(&mut inner, TapEvent::Event(Arc::new(event.clone())));
    }
}

fn publish(inner: &mut Inner, event: TapEvent) {
    inner
        .subscribers
        .retain(|s| match s.tx.try_send(event.clone()) {
            Ok(()) => true,
            Err(TrySendError::Full(lost)) => {
                match lost {
                    TapEvent::Samples { batch, .. } => {
                        s.drops.batches.fetch_add(1, Ordering::Relaxed);
                        s.drops
                            .ticks
                            .fetch_add(batch.ticks() as u64, Ordering::Relaxed);
                    }
                    _ => {
                        s.drops.events.fetch_add(1, Ordering::Relaxed);
                    }
                }
                true
            }
            Err(TrySendError::Disconnected(_)) => false,
        });
}

/// A subscriber's end of the tap; unsubscribes when dropped.
pub struct Subscription {
    rx: Receiver<TapEvent>,
    drops: Arc<Drops>,
    key: u64,
    tap: std::sync::Weak<Tap>,
}

impl Subscription {
    pub fn recv_timeout(&self, timeout: Duration) -> Result<TapEvent, RecvTimeoutError> {
        self.rx.recv_timeout(timeout)
    }

    pub fn try_recv(&self) -> Option<TapEvent> {
        self.rx.try_recv().ok()
    }

    pub fn drops(&self) -> &Arc<Drops> {
        &self.drops
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(tap) = self.tap.upgrade() {
            tap.lock().subscribers.retain(|s| s.key != self.key);
        }
    }
}

/// Wraps the UI's sink: everything goes to the tap first, then on to the UI.
pub struct TapSink {
    tap: Arc<Tap>,
    inner: Arc<dyn SessionSink>,
}

impl TapSink {
    pub fn new(tap: Arc<Tap>, inner: Arc<dyn SessionSink>) -> Self {
        Self { tap, inner }
    }
}

impl SessionSink for TapSink {
    fn frame(&self, bytes: Vec<u8>) -> bool {
        self.inner.frame(bytes)
    }

    fn event(&self, event: SessionEvent) {
        self.tap.event(&event);
        self.inner.event(event);
    }

    fn samples(&self, batch: &Arc<SampleBatch>) {
        self.tap.samples(batch.clone());
        self.inner.samples(batch);
    }

    fn started(&self, at: SystemTime) {
        self.tap.clock(at);
        self.inner.started(at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: u32, name: &str) -> WatchMeta {
        WatchMeta {
            id,
            name: name.into(),
            path: format!("app::{name}"),
            type_name: "f32".into(),
            unit: None,
        }
    }

    fn batch(ids: Vec<u32>, ticks: usize) -> Arc<SampleBatch> {
        let c = ids.len();
        Arc::new(SampleBatch {
            ids,
            times: (0..ticks).map(|i| i as f64).collect(),
            values: vec![1.0; ticks * c],
        })
    }

    #[test]
    fn names_are_unique_and_never_t() {
        let set = WatchSet::new(vec![meta(1, "x"), meta(2, "x"), meta(3, "t")]);
        let names: Vec<_> = set.watches().iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, ["x", "x#2", "t#3"]);
    }

    #[test]
    fn full_queue_drops_and_counts_without_blocking() {
        let tap = Tap::new();
        let (sub, _) = tap.subscribe(2);
        tap.set_watches(vec![meta(1, "a")]);
        for _ in 0..5 {
            tap.samples(batch(vec![1], 10));
        }
        // The watch set took one slot, one batch fit, four were dropped
        assert_eq!(sub.drops().batches(), 4);
        assert_eq!(sub.drops().ticks(), 40);
        assert!(matches!(sub.try_recv(), Some(TapEvent::Watches(_))));
        match sub.try_recv() {
            Some(TapEvent::Samples { watches, .. }) => assert_eq!(watches.watches()[0].name, "a"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn batches_carry_their_own_columns() {
        let tap = Tap::new();
        tap.set_watches(vec![meta(1, "a"), meta(2, "b")]);
        let (sub, snap) = tap.subscribe(8);
        assert_eq!(snap.watches.watches().len(), 2);
        // A batch still flushed for the old set after the list changed
        tap.set_watches(vec![meta(2, "b")]);
        tap.samples(batch(vec![1, 2], 1));
        let _ = sub.try_recv();
        match sub.try_recv() {
            Some(TapEvent::Samples { watches, .. }) => {
                let names: Vec<_> = watches.watches().iter().map(|w| &w.name).collect();
                assert_eq!(names, ["a", "b"]);
            }
            other => panic!("{other:?}"),
        }
        drop(sub);
        assert!(tap.lock().subscribers.is_empty());
    }
}
