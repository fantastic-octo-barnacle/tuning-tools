//! Link statistics. Read latency tracking is ported from datavis-rs
//! `ProbeStats` (MIT); the achieved rate is measured from tick timestamps
//! instead of being inferred from read time.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde::Serialize;

const WINDOW: usize = 200;

#[derive(Debug, Clone, Default)]
pub struct LinkStats {
    /// Whole-tick read times in microseconds, most recent last
    recent_read_us: VecDeque<u64>,
    /// Tick instants, most recent last
    recent_ticks: VecDeque<Instant>,
    pub ticks: u64,
    pub failed_regions: u64,
    pub bytes: u64,
    pub log_bytes: u64,
    pub frames_sent: u64,
    pub frames_dropped: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StatsSnapshot {
    pub target_hz: f64,
    pub achieved_hz: f64,
    pub read_avg_us: f64,
    pub read_max_us: u64,
    /// Max minus min over the recent window
    pub jitter_us: u64,
    pub skipped_ticks: u64,
    pub failed_regions: u64,
    pub regions: usize,
    pub values: usize,
    pub bytes_per_sec: f64,
    pub log_bytes: u64,
    pub frames_dropped: u64,
}

impl LinkStats {
    pub fn record_tick(&mut self, at: Instant, read: Duration, bytes: u64, failed_regions: u32) {
        self.ticks += 1;
        self.bytes += bytes;
        self.failed_regions += failed_regions as u64;
        self.recent_read_us.push_back(read.as_micros() as u64);
        self.recent_ticks.push_back(at);
        if self.recent_read_us.len() > WINDOW {
            self.recent_read_us.pop_front();
            self.recent_ticks.pop_front();
        }
    }

    /// Forget the rate window, e.g. after the watched set or rate changed.
    pub fn restart_window(&mut self) {
        self.recent_read_us.clear();
        self.recent_ticks.clear();
    }

    pub fn achieved_hz(&self) -> f64 {
        match (self.recent_ticks.front(), self.recent_ticks.back()) {
            (Some(first), Some(last)) if self.recent_ticks.len() > 1 => {
                let span = (*last - *first).as_secs_f64();
                if span > 0.0 {
                    (self.recent_ticks.len() - 1) as f64 / span
                } else {
                    0.0
                }
            }
            _ => 0.0,
        }
    }

    pub fn snapshot(
        &self,
        target_hz: f64,
        skipped_ticks: u64,
        regions: usize,
        values: usize,
    ) -> StatsSnapshot {
        let n = self.recent_read_us.len();
        let (min, max) = self
            .recent_read_us
            .iter()
            .fold((u64::MAX, 0), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        let avg = if n == 0 {
            0.0
        } else {
            self.recent_read_us.iter().sum::<u64>() as f64 / n as f64
        };
        let achieved = self.achieved_hz();
        let avg_bytes = if self.ticks == 0 {
            0.0
        } else {
            self.bytes as f64 / self.ticks as f64
        };
        StatsSnapshot {
            target_hz,
            achieved_hz: achieved,
            read_avg_us: avg,
            read_max_us: max,
            jitter_us: if n == 0 { 0 } else { max - min },
            skipped_ticks,
            failed_regions: self.failed_regions,
            regions,
            values,
            bytes_per_sec: avg_bytes * achieved,
            log_bytes: self.log_bytes,
            frames_dropped: self.frames_dropped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn achieved_rate_comes_from_tick_spacing() {
        let t0 = Instant::now();
        let mut s = LinkStats::default();
        for i in 0..11 {
            s.record_tick(
                t0 + Duration::from_millis(10 * i),
                Duration::from_micros(300 + i),
                8,
                0,
            );
        }
        let snap = s.snapshot(100.0, 0, 1, 2);
        assert!((snap.achieved_hz - 100.0).abs() < 1e-6);
        assert_eq!(snap.jitter_us, 10);
        assert_eq!(snap.read_max_us, 310);
    }
}
