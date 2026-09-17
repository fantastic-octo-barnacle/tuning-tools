//! Decoding the tuning table from a rustc-built ELF. The fixture is
//! `studio-dwarf/tests/fixtures/rust_embedded/src/bin/telemetry`.

use studio_carriers::{MemoryAccess, Result};
use studio_core::catalog::{Access, CellKind, ElfImage, TableLayout};
use studio_dwarf::ElfParser;

const ELF: &[u8] = include_bytes!("fixtures/rm_telemetry.elf");

fn fnv1a(s: &str) -> u32 {
    s.bytes().fold(0x811c_9dc5, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    })
}

fn layout() -> TableLayout {
    let elf = ElfParser::parse_bytes(ELF, "rm_telemetry.elf").unwrap();
    TableLayout::find(&elf)
        .unwrap()
        .expect("fixture declares a table")
}

#[test]
fn the_table_decodes_from_the_elf_alone() {
    let layout = layout();
    let catalog = layout.read(&mut ElfImage::parse(ELF).unwrap()).unwrap();
    assert_eq!(catalog.symbol, "telemetry::tuning::TABLE");

    let names: Vec<&str> = catalog.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "gimbal.pitch.angle.kp",
            "gimbal.pitch.angle.kd",
            "gimbal.pitch.angle_rad",
            "gimbal.pitch.offset",
            "robot.ticks",
            "robot.armed"
        ]
    );
    for e in &catalog.entries {
        assert_eq!(e.id, fnv1a(&e.name), "{}", e.name);
        assert_eq!(e.applied_address, e.requested_address + 4, "{}", e.name);
        assert_eq!(e.applied_address >> 28, 0x2, "{} cell is in RAM", e.name);
    }

    let kp = &catalog.entries[0];
    assert_eq!((kp.kind, kp.access), (CellKind::F32, Access::Live));
    assert_eq!(kp.unit, "1/s");
    assert_eq!(kp.default, 40.0);
    assert_eq!(
        (kp.min, kp.max, kp.max_step),
        (Some(0.0), Some(200.0), Some(0.2))
    );
    assert_eq!(catalog.entries[1].max_step, Some(0.01));

    let kinds: Vec<(CellKind, Access)> = catalog.entries[2..]
        .iter()
        .map(|e| (e.kind, e.access))
        .collect();
    assert_eq!(
        kinds,
        [
            (CellKind::F32, Access::ReadOnly),
            (CellKind::I32, Access::ReadOnly),
            (CellKind::U32, Access::ReadOnly),
            (CellKind::Bool, Access::ReadOnly)
        ]
    );
    assert!(catalog.entries[2..]
        .iter()
        .all(|e| e.min.is_none() && e.max_step.is_none()));
}

#[test]
fn the_initial_request_is_the_default() {
    let catalog = layout().read(&mut ElfImage::parse(ELF).unwrap()).unwrap();
    let kp = &catalog.entries[0];
    let mut image = ElfImage::parse(ELF).unwrap();
    let mut bits = [0; 4];
    image.read(kp.requested_address, &mut bits).unwrap();
    assert_eq!(f32::from_le_bytes(bits), 40.0);
}

/// The ELF image with some bytes overridden
struct Patched(ElfImage, u64, Vec<u8>);

impl MemoryAccess for Patched {
    fn read(&mut self, address: u64, buf: &mut [u8]) -> Result<()> {
        self.0.read(address, buf)?;
        for (i, b) in buf.iter_mut().enumerate() {
            let at = address + i as u64;
            if at >= self.1 && at < self.1 + self.2.len() as u64 {
                *b = self.2[(at - self.1) as usize];
            }
        }
        Ok(())
    }

    fn write(&mut self, address: u64, data: &[u8]) -> Result<()> {
        self.0.write(address, data)
    }
}

#[test]
fn a_table_with_the_wrong_magic_or_version_is_refused() {
    let layout = layout();
    let image = || ElfImage::parse(ELF).unwrap();
    // rustc orders the fields, so find them by value
    let mut table = [0u8; 16];
    image().read(layout.address, &mut table).unwrap();
    let find = |word: u32| {
        (0..table.len() - 3)
            .step_by(4)
            .find(|&i| table[i..i + 4] == word.to_le_bytes())
            .map(|i| layout.address + i as u64)
            .unwrap()
    };
    let magic_at = find(u32::from_le_bytes(*b"RMTT"));
    let version_at = find(1);

    let err = layout
        .read(&mut Patched(image(), magic_at, b"XXXX".to_vec()))
        .unwrap_err();
    assert!(err.to_string().contains("magic"), "{err}");

    let err = layout
        .read(&mut Patched(
            image(),
            version_at,
            2u32.to_le_bytes().to_vec(),
        ))
        .unwrap_err();
    assert!(err.to_string().contains("format version 2"), "{err}");
}

#[test]
fn an_elf_without_a_table_has_no_layout() {
    let other = include_bytes!("../../studio-dwarf/tests/fixtures/rust_v0.elf");
    let elf = ElfParser::parse_bytes(other, "rust_v0.elf").unwrap();
    assert!(TableLayout::find(&elf).unwrap().is_none());
}
