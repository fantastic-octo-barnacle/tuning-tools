# Tuning Tools

Live variable watch, tune and telemetry studio for RoboMaster firmware.
Desktop app: Tauri 2 host in Rust, React + TypeScript frontend.

This is the successor to `herkules-tools` and `datavis-rs`. The design,
the reasoning behind it, and the build order are in [FRAME.md](FRAME.md).
Architecture notes on the projects it draws from are in
[docs/references/](docs/references/).

## Status

Scaffold only. Nothing beyond the Tauri template runs yet.

## Develop

```bash
npm install
npm run tauri dev        # desktop app with native probe/serial access
npm run dev              # frontend only, no hardware
cargo check --manifest-path src-tauri/Cargo.toml
```

Requires Rust stable, Node 20+, and the platform Tauri prerequisites.

## Layout

```
src/            React frontend (widgets, layout, data plane consumer)
src-tauri/      Tauri host: commands, event/channel bridge to studio-core
crates/         Rust workspace crates (planned; see FRAME.md)
docs/           design records and reference notes
```
