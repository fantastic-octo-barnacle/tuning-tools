//! Read planning: coalesce watched values into as few memory reads as possible,
//! once per change of the watched set, then decode each value from its region.
//!
//! The coalescing rule is ported from datavis-rs `ReadManager` (MIT): sort by
//! address and merge a value into the current region when it starts within
//! `gap` bytes of the region end and the merged region stays under `max_len`.

use serde::Deserialize;
use studio_carriers::MemoryAccess;
use studio_dwarf::VariableType;

/// Bytes of unrelated memory worth reading to save a round trip.
pub const DEFAULT_GAP: u64 = 64;
/// Largest single read; keeps one slow region from stalling the whole tick.
pub const DEFAULT_MAX_LEN: u64 = 512;

/// One value to sample.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadItem {
    /// Caller-chosen id, echoed in frames
    pub id: u32,
    pub address: u64,
    pub scalar: VariableType,
    /// Bitfield position counted from `address` (DWARF 4 `DW_AT_data_bit_offset`)
    pub bit_offset: Option<u64>,
    pub bit_size: Option<u64>,
}

impl ReadItem {
    /// Bytes of target memory the value occupies, as `(start, len)`.
    pub fn span(&self) -> Option<(u64, u64)> {
        match (self.bit_offset, self.bit_size) {
            (Some(bit), Some(bits)) if bits > 0 => {
                let first = bit / 8;
                let len = (bit % 8 + bits).div_ceil(8);
                Some((self.address + first, len))
            }
            _ => scalar_len(self.scalar).map(|len| (self.address, len)),
        }
    }
}

/// Byte width of a plottable scalar; `None` for raw byte blobs.
pub fn scalar_len(scalar: VariableType) -> Option<u64> {
    Some(match scalar {
        VariableType::U8 | VariableType::I8 | VariableType::Bool => 1,
        VariableType::U16 | VariableType::I16 => 2,
        VariableType::U32 | VariableType::I32 | VariableType::F32 => 4,
        VariableType::U64 | VariableType::I64 | VariableType::F64 => 8,
        VariableType::Raw(_) => return None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    pub address: u64,
    pub len: u64,
}

#[derive(Debug, Clone)]
struct Slot {
    region: usize,
    /// Byte offset of the value's span inside its region
    offset: usize,
    len: usize,
}

/// Plan for one watched set. Column `i` of the output is `items()[i]`.
#[derive(Debug, Clone, Default)]
pub struct ReadPlan {
    items: Vec<ReadItem>,
    slots: Vec<Option<Slot>>,
    regions: Vec<Region>,
    buffer: Vec<u8>,
}

/// What one [`ReadPlan::sample`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SampleOutcome {
    pub regions_ok: u32,
    pub regions_failed: u32,
    pub bytes: u64,
    /// First read error of the tick, for the status stream
    pub first_error: Option<String>,
}

impl ReadPlan {
    pub fn new(items: Vec<ReadItem>) -> Self {
        Self::with_limits(items, DEFAULT_GAP, DEFAULT_MAX_LEN)
    }

    pub fn with_limits(items: Vec<ReadItem>, gap: u64, max_len: u64) -> Self {
        let mut order: Vec<(usize, u64, u64)> = items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| item.span().map(|(start, len)| (i, start, len)))
            .collect();
        order.sort_by_key(|&(_, start, len)| (start, len));

        let mut regions: Vec<Region> = Vec::new();
        let mut slots: Vec<Option<Slot>> = vec![None; items.len()];
        for (i, start, len) in order {
            let end = start + len;
            let merge = regions.last().is_some_and(|r| {
                let r_end = r.address + r.len;
                start <= r_end + gap && end.max(r_end) - r.address <= max_len
            });
            if merge {
                let r = regions.last_mut().expect("checked above");
                r.len = end.max(r.address + r.len) - r.address;
            } else {
                regions.push(Region {
                    address: start,
                    len,
                });
            }
            let region = regions.len() - 1;
            slots[i] = Some(Slot {
                region,
                offset: (start - regions[region].address) as usize,
                len: len as usize,
            });
        }
        let largest = regions.iter().map(|r| r.len).max().unwrap_or(0) as usize;
        Self {
            items,
            slots,
            regions,
            buffer: vec![0; largest],
        }
    }

    pub fn items(&self) -> &[ReadItem] {
        &self.items
    }

    pub fn regions(&self) -> &[Region] {
        &self.regions
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Read every region and write one value per item into `out` (NaN when the
    /// region failed or the item cannot be decoded).
    pub fn sample(&mut self, memory: &mut dyn MemoryAccess, out: &mut Vec<f64>) -> SampleOutcome {
        out.clear();
        out.resize(self.items.len(), f64::NAN);
        let mut outcome = SampleOutcome::default();
        // Regions are few; a small index keeps the slot loop allocation-free
        for (region_index, region) in self.regions.iter().enumerate() {
            let buf = &mut self.buffer[..region.len as usize];
            if let Err(e) = memory.read(region.address, buf) {
                outcome.regions_failed += 1;
                outcome.first_error.get_or_insert_with(|| e.to_string());
                continue;
            }
            outcome.regions_ok += 1;
            outcome.bytes += region.len;
            for ((item, slot), value) in self.items.iter().zip(&self.slots).zip(out.iter_mut()) {
                let Some(slot) = slot.as_ref().filter(|s| s.region == region_index) else {
                    continue;
                };
                *value = decode(item, &buf[slot.offset..slot.offset + slot.len]);
            }
        }
        outcome
    }
}

/// Decode a little-endian value (or bitfield) to f64.
pub fn decode(item: &ReadItem, bytes: &[u8]) -> f64 {
    if let (Some(bit), Some(bits)) = (item.bit_offset, item.bit_size) {
        if bits == 0 || bits > 64 {
            return f64::NAN;
        }
        let mut raw = 0u128;
        for (i, b) in bytes.iter().enumerate() {
            raw |= (*b as u128) << (8 * i);
        }
        let field = ((raw >> (bit % 8)) & ((1u128 << bits) - 1)) as u64;
        let signed = matches!(
            item.scalar,
            VariableType::I8 | VariableType::I16 | VariableType::I32 | VariableType::I64
        );
        return if signed && bits < 64 && field >> (bits - 1) & 1 == 1 {
            (field as i64 - (1i64 << bits)) as f64
        } else {
            field as f64
        };
    }
    let arr = |n: usize| -> Option<[u8; 8]> {
        let mut a = [0u8; 8];
        a[..n].copy_from_slice(bytes.get(..n)?);
        Some(a)
    };
    let Some(a) = scalar_len(item.scalar).and_then(|n| arr(n as usize)) else {
        return f64::NAN;
    };
    match item.scalar {
        VariableType::U8 => a[0] as f64,
        VariableType::I8 => a[0] as i8 as f64,
        VariableType::Bool => (a[0] != 0) as u8 as f64,
        VariableType::U16 => u16::from_le_bytes([a[0], a[1]]) as f64,
        VariableType::I16 => i16::from_le_bytes([a[0], a[1]]) as f64,
        VariableType::U32 => u32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f64,
        VariableType::I32 => i32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f64,
        VariableType::F32 => f32::from_le_bytes([a[0], a[1], a[2], a[3]]) as f64,
        VariableType::U64 => u64::from_le_bytes(a) as f64,
        VariableType::I64 => i64::from_le_bytes(a) as f64,
        VariableType::F64 => f64::from_le_bytes(a),
        VariableType::Raw(_) => f64::NAN,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_carriers::mock::MockLink;

    fn item(id: u32, address: u64, scalar: VariableType) -> ReadItem {
        ReadItem {
            id,
            address,
            scalar,
            bit_offset: None,
            bit_size: None,
        }
    }

    #[test]
    fn neighbours_share_a_region() {
        let plan = ReadPlan::new(vec![
            item(1, 0x2000_0010, VariableType::F32),
            item(2, 0x2000_0000, VariableType::U32),
            item(3, 0x2000_0100, VariableType::U8),
        ]);
        assert_eq!(
            plan.regions(),
            &[
                Region {
                    address: 0x2000_0000,
                    len: 0x14
                },
                Region {
                    address: 0x2000_0100,
                    len: 1
                },
            ]
        );
    }

    #[test]
    fn max_len_splits_regions() {
        let plan = ReadPlan::with_limits(
            vec![
                item(1, 0x100, VariableType::U32),
                item(2, 0x104, VariableType::U32),
                item(3, 0x108, VariableType::U32),
            ],
            64,
            8,
        );
        assert_eq!(plan.regions().len(), 2);
    }

    #[test]
    fn samples_decode_in_item_order() {
        let mock = MockLink::new();
        mock.poke(0x2000_0000, &42u32.to_le_bytes());
        mock.poke(0x2000_0004, &(-1.5f32).to_le_bytes());
        mock.poke(0x2000_0008, &(-7i16).to_le_bytes());
        let mut plan = ReadPlan::new(vec![
            item(3, 0x2000_0008, VariableType::I16),
            item(1, 0x2000_0000, VariableType::U32),
            item(2, 0x2000_0004, VariableType::F32),
        ]);
        let mut out = Vec::new();
        let mut link = mock.clone();
        let outcome = plan.sample(&mut link, &mut out);
        assert_eq!(out, vec![-7.0, 42.0, -1.5]);
        assert_eq!(outcome.regions_ok, 1);
        assert_eq!(mock.read_calls(), 1);
    }

    #[test]
    fn failed_region_yields_nan_only_for_its_items() {
        let mock = MockLink::new();
        mock.poke(0x100, &[5]);
        mock.fail_reads(0x9000, 0x9001);
        let mut plan = ReadPlan::new(vec![
            item(1, 0x100, VariableType::U8),
            item(2, 0x9000, VariableType::U8),
        ]);
        let mut out = Vec::new();
        let outcome = plan.sample(&mut mock.clone(), &mut out);
        assert_eq!(out[0], 5.0);
        assert!(out[1].is_nan());
        assert_eq!((outcome.regions_ok, outcome.regions_failed), (1, 1));
        assert!(outcome.first_error.is_some());
    }

    #[test]
    fn bitfields_decode_with_sign() {
        // bits 3..8 of byte 1 and bit 0 of byte 2: 6-bit field at bit 11
        let field = ReadItem {
            id: 1,
            address: 0,
            scalar: VariableType::I32,
            bit_offset: Some(11),
            bit_size: Some(6),
        };
        assert_eq!(field.span(), Some((1, 2)));
        // -3 in 6 bits = 0b111101, stored from bit 3 of the span
        let raw: u16 = 0b111101 << 3;
        assert_eq!(decode(&field, &raw.to_le_bytes()), -3.0);
        let unsigned = ReadItem {
            scalar: VariableType::U32,
            ..field
        };
        assert_eq!(decode(&unsigned, &raw.to_le_bytes()), 61.0);
    }

    #[test]
    fn raw_items_are_not_read() {
        let plan = ReadPlan::new(vec![item(1, 0x100, VariableType::Raw(12))]);
        assert!(plan.regions().is_empty());
    }
}
