//! Tuning over memory access: sample every catalog value's request and applied
//! cells, and write requests once the target is known to run the open build.

use serde::Serialize;
use studio_carriers::MemoryAccess;

use crate::catalog::{widen, Catalog, CatalogEntry, CellKind, TableLayout};
use crate::plan::ReadPlan;

/// Whether the target's own table matches the ELF's. Writes wait for `Matches`:
/// cell addresses from another build would land on unrelated memory.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum CatalogCheck {
    Checking,
    Matches,
    Differs { message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TuneValue {
    pub id: u32,
    /// `None` when the cell could not be read
    pub requested: Option<f64>,
    pub applied: Option<f64>,
}

pub struct Tuner {
    layout: TableLayout,
    catalog: Catalog,
    plan: ReadPlan,
    row: Vec<f64>,
    check: CatalogCheck,
}

impl Tuner {
    pub fn new(layout: TableLayout, catalog: Catalog) -> Self {
        let items = catalog
            .entries
            .iter()
            .enumerate()
            .flat_map(|(i, e)| {
                let i = i as u32;
                [e.requested_item(2 * i), e.applied_item(2 * i + 1)]
            })
            .collect();
        Self {
            layout,
            catalog,
            plan: ReadPlan::new(items),
            row: Vec::new(),
            check: CatalogCheck::Checking,
        }
    }

    pub fn check(&self) -> &CatalogCheck {
        &self.check
    }

    /// Compare the target's table with the ELF's until it is decided. A read
    /// that fails leaves it undecided, to try again; the error is returned.
    pub fn verify(&mut self, memory: &mut dyn MemoryAccess) -> Option<String> {
        if self.check != CatalogCheck::Checking {
            return None;
        }
        match self.layout.read(memory) {
            Ok(live) if live.entries == self.catalog.entries => {
                self.check = CatalogCheck::Matches;
                None
            }
            Ok(_) => {
                self.check = differs("its table lists different values");
                None
            }
            Err(crate::catalog::CatalogError::Memory(e)) => Some(e.to_string()),
            Err(e) => {
                self.check = differs(&e.to_string());
                None
            }
        }
    }

    pub fn sample(&mut self, memory: &mut dyn MemoryAccess) -> Vec<TuneValue> {
        self.plan.sample(memory, &mut self.row);
        self.catalog
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| TuneValue {
                id: e.id,
                requested: cell(e, self.row[2 * i]),
                applied: cell(e, self.row[2 * i + 1]),
            })
            .collect()
    }

    /// Write a request for `value` into the cell of entry `id`.
    pub fn request(
        &mut self,
        memory: &mut dyn MemoryAccess,
        id: u32,
        value: f64,
    ) -> Result<(), String> {
        match &self.check {
            CatalogCheck::Matches => {}
            CatalogCheck::Checking => {
                return Err("still checking that the target runs the open ELF".into())
            }
            CatalogCheck::Differs { message } => return Err(message.clone()),
        }
        let entry = self
            .catalog
            .entry(id)
            .ok_or_else(|| format!("no value with id {id:#010x} in this build"))?;
        let bits = entry.request_bits(value)?;
        memory
            .write(entry.requested_address, &bits.to_le_bytes())
            .map_err(|e| e.to_string())
    }
}

fn differs(reason: &str) -> CatalogCheck {
    CatalogCheck::Differs {
        message: format!(
            "The target is not running the open ELF ({reason}). Flash this build or open the one it runs before tuning."
        ),
    }
}

fn cell(entry: &CatalogEntry, sampled: f64) -> Option<f64> {
    if sampled.is_nan() {
        return None;
    }
    Some(if entry.kind == CellKind::F32 && sampled.is_finite() {
        widen(sampled as f32)
    } else {
        sampled
    })
}
