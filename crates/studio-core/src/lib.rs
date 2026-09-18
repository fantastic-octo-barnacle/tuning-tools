//! Acquisition core: read planning, the sampling session that owns a carrier,
//! probe statistics, sample frames for the webview and defmt log decoding.

pub mod catalog;
pub mod frame;
pub mod link;
pub mod log;
pub mod plan;
pub mod rtt_tuning;
pub mod schedule;
pub mod session;
pub mod stats;
pub mod tap;
pub mod tune;
pub mod wire;

pub use frame::SampleBatch;
pub use plan::{ReadItem, ReadPlan};
pub use session::{Session, SessionCommand, SessionEvent, SessionSink};
pub use tap::{Tap, TapEvent, TapSink, WatchMeta, WatchSet};
