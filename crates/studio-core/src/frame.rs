//! Binary sample frames sent to the webview about 30 times a second.
//!
//! Layout, little-endian, every field 8-byte aligned so the webview can view
//! the columns as `Float64Array`s without copying:
//!
//! ```text
//! u32 magic "TTS1" | u32 ticks n | u32 columns c | u32 reserved
//! f64 time[n]                          seconds since the session started
//! c times: u32 id | u32 reserved | f64 value[n]    NaN where a read failed
//! ```

pub const MAGIC: u32 = u32::from_le_bytes(*b"TTS1");

/// Accumulates ticks for the current watched set until flushed.
#[derive(Debug, Default)]
pub struct FrameBuilder {
    ids: Vec<u32>,
    times: Vec<f64>,
    /// Row-major: one row of `ids.len()` values per tick
    values: Vec<f64>,
}

impl FrameBuilder {
    /// Start collecting for `ids`. Call [`FrameBuilder::flush`] first when ticks are pending.
    pub fn reset(&mut self, ids: Vec<u32>) {
        self.ids = ids;
        self.times.clear();
        self.values.clear();
    }

    pub fn push(&mut self, time: f64, row: &[f64]) {
        debug_assert_eq!(row.len(), self.ids.len());
        self.times.push(time);
        self.values.extend_from_slice(row);
    }

    pub fn is_empty(&self) -> bool {
        self.times.is_empty()
    }

    /// Take pending ticks as a batch, leaving the builder empty for the same
    /// ids; `None` when there are none.
    pub fn take(&mut self) -> Option<SampleBatch> {
        if self.times.is_empty() {
            return None;
        }
        Some(SampleBatch {
            ids: self.ids.clone(),
            times: std::mem::take(&mut self.times),
            values: std::mem::take(&mut self.values),
        })
    }

    /// Encode pending ticks and clear them; `None` when there are none.
    pub fn flush(&mut self) -> Option<Vec<u8>> {
        self.take().map(|batch| batch.encode())
    }
}

/// Ticks flushed together: the columns of one frame, before encoding.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SampleBatch {
    /// Watch id of each column
    pub ids: Vec<u32>,
    /// Seconds since the session started, one per tick
    pub times: Vec<f64>,
    /// Row-major: one row of `ids.len()` values per tick, NaN where a read failed
    pub values: Vec<f64>,
}

impl SampleBatch {
    pub fn ticks(&self) -> usize {
        self.times.len()
    }

    /// The values of tick `row`, in `ids` order
    pub fn row(&self, row: usize) -> &[f64] {
        let c = self.ids.len();
        &self.values[row * c..(row + 1) * c]
    }

    /// Column `col` over every tick
    pub fn column(&self, col: usize) -> impl Iterator<Item = f64> + '_ {
        let c = self.ids.len();
        (0..self.times.len()).map(move |row| self.values[row * c + col])
    }

    /// The TTS1 frame for these ticks.
    pub fn encode(&self) -> Vec<u8> {
        let n = self.times.len();
        let c = self.ids.len();
        let mut out = Vec::with_capacity(16 + 8 * n + c * (8 + 8 * n));
        for word in [MAGIC, n as u32, c as u32, 0] {
            out.extend_from_slice(&word.to_le_bytes());
        }
        for t in &self.times {
            out.extend_from_slice(&t.to_le_bytes());
        }
        for (col, id) in self.ids.iter().enumerate() {
            out.extend_from_slice(&id.to_le_bytes());
            out.extend_from_slice(&0u32.to_le_bytes());
            for v in self.column(col) {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        out
    }
}

/// Decoded frame, for tests and tools.
#[derive(Debug, PartialEq)]
pub struct DecodedFrame {
    pub times: Vec<f64>,
    pub columns: Vec<(u32, Vec<f64>)>,
}

pub fn decode(bytes: &[u8]) -> Option<DecodedFrame> {
    let u32_at = |at: usize| Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    let f64_at = |at: usize| Some(f64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?));
    if u32_at(0)? != MAGIC {
        return None;
    }
    let n = u32_at(4)? as usize;
    let c = u32_at(8)? as usize;
    let mut at = 16;
    let times = (0..n)
        .map(|i| f64_at(at + 8 * i))
        .collect::<Option<Vec<_>>>()?;
    at += 8 * n;
    let mut columns = Vec::with_capacity(c);
    for _ in 0..c {
        let id = u32_at(at)?;
        at += 8;
        let values = (0..n)
            .map(|i| f64_at(at + 8 * i))
            .collect::<Option<Vec<_>>>()?;
        at += 8 * n;
        columns.push((id, values));
    }
    Some(DecodedFrame { times, columns })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_columns() {
        let mut b = FrameBuilder::default();
        assert!(b.flush().is_none());
        b.reset(vec![7, 9]);
        b.push(0.0, &[1.0, 2.0]);
        b.push(0.01, &[3.0, f64::NAN]);
        let bytes = b.flush().unwrap();
        assert_eq!(bytes.len() % 8, 0);
        let frame = decode(&bytes).unwrap();
        assert_eq!(frame.times, vec![0.0, 0.01]);
        assert_eq!(frame.columns[0], (7, vec![1.0, 3.0]));
        assert_eq!(frame.columns[1].1[0], 2.0);
        assert!(frame.columns[1].1[1].is_nan());
        assert!(b.is_empty());
    }
}
