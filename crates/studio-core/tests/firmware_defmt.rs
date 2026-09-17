//! Needs a real firmware ELF: `STUDIO_DWARF_ELF=<path> cargo test -p studio-core -- --ignored`

use studio_core::log::LogDecoder;

#[test]
#[ignore = "needs STUDIO_DWARF_ELF"]
fn firmware_has_a_defmt_table() {
    let path = std::env::var("STUDIO_DWARF_ELF").expect("set STUDIO_DWARF_ELF");
    let elf = std::fs::read(path).unwrap();
    let decoder = LogDecoder::spawn(elf, |_| {}).expect("table parses");
    assert!(decoder.is_some(), "firmware ELF has no defmt table");
}
