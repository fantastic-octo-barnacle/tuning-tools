//! Print the tuning table a firmware ELF declares, decoded from the file.
//!
//! `cargo run -p studio-core --example catalog -- path/to/firmware`

use studio_core::catalog::{ElfImage, TableLayout};
use studio_dwarf::ElfParser;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("usage: catalog <elf>")?;
    let elf = ElfParser::parse(&path)?;
    let Some(layout) = TableLayout::find(&elf)? else {
        println!("{path} declares no tuning table");
        return Ok(());
    };
    let mut image = ElfImage::parse(&std::fs::read(&path)?)?;
    let catalog = layout.read(&mut image)?;
    println!("{} at {:#010x}", catalog.symbol, catalog.address);
    for e in &catalog.entries {
        let range = match (e.min, e.max, e.max_step) {
            (Some(min), Some(max), Some(step)) => format!("{min}..={max} step {step}"),
            _ => String::new(),
        };
        println!(
            "  {:08x} {:<36} {:?} {:?} default {} {} [{}] cell {:#010x}",
            e.id, e.name, e.kind, e.access, e.default, e.unit, range, e.applied_address
        );
    }
    Ok(())
}
