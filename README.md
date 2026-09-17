# Tuning Tools

Live variable watch, tune and telemetry studio for RoboMaster firmware.
Desktop app: Tauri 2 host in Rust, React + TypeScript frontend.

This is the successor to `herkules-tools` and `datavis-rs`. The design,
the reasoning behind it, and the build order are in [FRAME.md](FRAME.md).
Architecture notes on the projects it draws from are in
[docs/references/](docs/references/).

## Status

M1. `studio-dwarf` parses C, C++ and Rust firmware ELFs (symbols, DWARF
types, Rust enums with data, rebuild diff). The app browses statics as a
module tree, attaches to a running target through a debug probe (probe-rs,
no halt, no reset), samples watched numbers on absolute deadlines, plots them,
and shows the firmware's defmt log from RTT. No firmware change is needed.
Writing values and the framed protocol come in M2 and M3.

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

Check a probe and board without the app (prints values and log lines):

```bash
cargo run -p studio-core --example watch -- --elf path/to/firmware --list
cargo run -p studio-core --example watch -- --elf path/to/firmware --chip STM32H723VG <static path>...
cargo run --release -p studio-carriers --example probe_bench -- STM32H723VG   # raw SWD read latency
```

Requires Rust stable, Node 20+, and the platform Tauri prerequisites.

## Layout

```
src/            React frontend (widgets, layout, data plane consumer)
src-tauri/      Tauri host: commands, event/channel bridge to studio-core
crates/
  studio-dwarf     ELF symbols and DWARF types
  studio-carriers  probe-rs memory access and RTT, mock target
  studio-core      read planner, session thread, stats, sample frames, defmt
docs/           design records and reference notes
```
