# Tuning Tools

Live variable watch, tune and telemetry studio for RoboMaster firmware.
Desktop app: Tauri 2 host in Rust, React + TypeScript frontend.

This is the successor to `herkules-tools` and `datavis-rs`. The design,
the reasoning behind it, and the build order are in [FRAME.md](FRAME.md).
Architecture notes on the projects it draws from are in
[docs/references/](docs/references/).

## Status

M0 in progress. `studio-dwarf` parses C, C++ and Rust firmware ELFs (symbols,
DWARF types, Rust enums with data, rebuild diff). The app opens an ELF and
browses its statics as a module tree with addresses, sizes and types.
No probe or serial connection yet.

## Develop

```bash
npm install
npm run tauri dev        # desktop app with native probe/serial access
npm run dev              # frontend only, no hardware
cargo test --workspace   # parser tests against fixtures in crates/studio-dwarf/tests
```

Open an ELF at startup instead of through the file dialog:

```bash
TUNING_TOOLS_ELF=path/to/firmware npm run tauri dev
```

Check the parser against a real firmware build:

```bash
STUDIO_DWARF_ELF=path/to/firmware cargo test -p studio-dwarf -- --ignored
```

Requires Rust stable, Node 20+, and the platform Tauri prerequisites.

## Layout

```
src/            React frontend (widgets, layout, data plane consumer)
src-tauri/      Tauri host: commands, event/channel bridge to studio-core
crates/         Rust workspace crates: studio-dwarf (ELF/DWARF); more planned, see FRAME.md
docs/           design records and reference notes
```
