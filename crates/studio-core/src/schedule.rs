//! Absolute-deadline ticker. Deadlines are `start + n * period`, so the rate does
//! not drift by the time each read takes; ticks that are already late are
//! skipped and counted rather than bunched up.

use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct Ticker {
    start: Instant,
    period: Duration,
    next: u64,
    skipped: u64,
}

impl Ticker {
    pub fn new(start: Instant, period: Duration) -> Self {
        assert!(!period.is_zero(), "ticker period must be positive");
        Self {
            start,
            period,
            next: 0,
            skipped: 0,
        }
    }

    pub fn from_hz(start: Instant, hz: f64) -> Self {
        Self::new(start, Duration::from_secs_f64(1.0 / hz))
    }

    pub fn period(&self) -> Duration {
        self.period
    }

    pub fn deadline(&self) -> Instant {
        self.start + self.period * self.next as u32
    }

    /// True when a tick is due at `now`; advances past it. More than one whole
    /// period late counts the missed ticks in [`Ticker::skipped`].
    pub fn poll(&mut self, now: Instant) -> bool {
        if now < self.deadline() {
            return false;
        }
        let due = (now - self.start).as_nanos() / self.period.as_nanos();
        let due = due as u64;
        self.skipped += due.saturating_sub(self.next);
        self.next = due + 1;
        true
    }

    /// Ticks missed because a previous tick ran past its successor's deadline.
    pub fn skipped(&self) -> u64 {
        self.skipped
    }
}

/// The OS sleep ends this early and the rest is spun, for sub-millisecond deadlines.
const SPIN: Duration = Duration::from_micros(500);

/// Sleep until `deadline`, returning at most a few tens of microseconds late.
pub fn sleep_until(deadline: Instant) {
    loop {
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        let left = deadline - now;
        if left > SPIN {
            std::thread::sleep(left - SPIN);
        } else {
            std::thread::yield_now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_on_absolute_deadlines() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut t = Ticker::new(t0, ms(10));
        assert!(t.poll(t0));
        assert!(!t.poll(t0 + ms(9)));
        // Late by 3 ms: next deadline is still 20 ms, not 13 + 10
        assert!(t.poll(t0 + ms(13)));
        assert_eq!(t.deadline(), t0 + ms(20));
        assert_eq!(t.skipped(), 0);
    }

    #[test]
    fn counts_skipped_ticks() {
        let t0 = Instant::now();
        let mut t = Ticker::new(t0, Duration::from_millis(10));
        assert!(t.poll(t0));
        assert!(t.poll(t0 + Duration::from_millis(45)));
        // Deadlines 10, 20, 30 were missed; 40 is the one served
        assert_eq!(t.skipped(), 3);
        assert_eq!(t.deadline(), t0 + Duration::from_millis(50));
    }
}
