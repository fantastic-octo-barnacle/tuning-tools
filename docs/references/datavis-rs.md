# datavis-rs

Source: https://github.com/hxyulin/datavis-rs — MIT — Rust 2021, eframe/egui 0.33 + egui_dock 0.18 + egui_plot 0.34, probe-rs 0.31, gimli 0.32 + object 0.38, rhai 1.23, crossbeam-channel — last commit `cb4c641` 2026-05-20 — 33.2k lines under `src/` (28.1k excluding inline `#[cfg(test)]`), 6 integration test files, 5 ELF fixtures, 1 criterion bench. Local clone used for this note: `scratchpad/datavis-rs`.

## Purpose

Desktop SWD variable plotter: connect a probe-rs (or OpenOCD) probe, load an ELF, pick globals from DWARF, poll them at up to ~1 kHz without halting the core, plot them, optionally transform them with Rhai, write values back, record to CSV. Two subsystems share one worker thread: a high-rate "Plot" poll and a low-rate Keil-style "Live Watch" tree (`src/lib.rs:28-39`).

## Module map

| Path | Responsibility |
|---|---|
| `src/backend/mod.rs` | `BackendCommand`/`BackendMessage` enums (105-152, 238-271), `FrontendReceiver` handle (274-382), `SwdBackend::new` builds the channel pair (398-417) |
| `src/backend/worker.rs` | `BackendWorker` loop: commands, poll, converters, stats, watch tick, rate limit (182-223, 547-671) |
| `src/backend/probe_trait.rs` | `DebugProbe` trait (163-234), `ProbeStats` rolling-window latency (17-148) |
| `src/backend/probe.rs` | probe-rs impl; AP0 memory interface (375-383), bulk `read_variables` (446-593), writes (632-678) |
| `src/backend/openocd/*` | OpenOCD subprocess + TCL-over-TCP impl (`probe.rs`, `process.rs`, `tcl_client.rs`, `chip_map.rs`); per-variable reads, no bulk (`probe.rs:171-200`) |
| `src/backend/mock_probe.rs`, `mock_fault.rs` | feature-gated pattern-generating probe for tests |
| `src/backend/read_manager.rs` | `ReadManager` address coalescer (39-229), `DependentReadPlanner` two-stage pointer reads (286-391) |
| `src/backend/watch_scheduler.rs` | Live Watch leaf poller with pointer cache (19-186) |
| `src/backend/elf_parser.rs` | symtab + dynsym via `object`, demangling, `ElfInfo` query API, DWARF merge by address then name (443-520) |
| `src/backend/dwarf_parser.rs` | single-pass gimli DIE walk into `TypeTable`, location-expression evaluation, `VariableStatus` (28-55) |
| `src/backend/type_table.rs` | index-based `TypeId` table, `TypeDef`, `TypeHandle` (957-1125) |
| `src/backend/converter_engine.rs` | per-variable compiled Rhai + prev-state (22-178) |
| `src/scripting/{mod,engine}.rs` | Rhai engine, limits, builtin DSP functions, script cache |
| `src/types.rs` | `Variable`, `VariableType`, `PointerMetadata/Runtime`, `VariableData` ring buffer, `CollectionStats` |
| `src/watch/mod.rs` | Live Watch domain: `WatchRoot`, `WatchLeafRead`, `walk_root` tree walk (184-221) |
| `src/frontend/mod.rs` | `DataVisApp` (2.5k lines): message drain (400-499), `handle_action` (510+), ELF reload diff (1030-1099), eframe `update` (2088+) |
| `src/frontend/state.rs` | `SharedState` borrow bundle (69-96), `AppAction` enum (106-204) |
| `src/frontend/pane_trait.rs`, `pane_registry.rs` | `Pane` vtable trait; 4 registered pane kinds |
| `src/frontend/panes/*` | `time_series`, `live_watch`, `variable_list`, `recorder` |
| `src/frontend/plot.rs` | `PlotView`, `PlotStatistics`, `PlotCursor` over egui_plot |
| `src/config/{mod,settings,ui_session}.rs` | `AppState`, `ProjectFile`, `AppConfig`, `RuntimeSettings`, `UiSessionState` |
| `src/session/*` | recorder/player for captured frames, CSV export |
| `src/i18n.rs`, `locales/*.yaml` | rust-i18n, en + zh-CN |

```
 eframe UI thread                       backend worker thread (std::thread)
 ┌──────────────────────────┐            ┌──────────────────────────────┐
 │ DataVisApp               │  bounded   │ BackendWorker                │
 │  panes → Vec<AppAction>  │──256──────▶│  process_commands            │
 │  handle_action           │ Command    │  DependentReadPlanner        │
 │  process_backend_messages│◀─10_000───│  probe.read_variables ──┐    │
 │  Topics (ring buffers)   │ Message    │  ConverterEngine (Rhai)  │    │
 └──────────────────────────┘            │  WatchScheduler          ▼    │
                                         │  Box<dyn DebugProbe>  probe-rs│
                                         └──────────────────────────────┘
```

## Acquisition path

1. UI edge: opening the first `TimeSeries` pane sends `StartCollection` (`src/frontend/mod.rs:2098-2110`); worker resumes the core and warns if still halted (`worker.rs:408-442`).
2. Per loop iteration `poll_variables` (`worker.rs:547-671`): one host timestamp `start_time.elapsed()` per poll (line 548) is stamped on every variable in the batch. Timestamps are host-side and relative to `StartCollection`/`ClearData`; nothing target-side.
3. `DependentReadPlanner::plan_reads` splits pointer roots from data vars (`read_manager.rs:308-335`); pointer roots are re-read only when their own `pointer_poll_rate_hz` elapses (338-352). `resolve_dependent_addresses` rewrites child addresses as `cached_ptr + offset` (243-278). One-tick lag by design.
4. `ProbeBackend::read_variables` (`probe.rs:446-593`): acquires the ARM AP0 memory interface once per poll (`v1_with_default_dp(0)`, 378), builds a `ReadManager` from config, then `plan_reads` sorts by address and merges neighbours when `addr <= current_end + gap_threshold` and merged size `<= max_read_size` (`read_manager.rs:126-173`). One `memory.read` per region; `extract_value` slices and `parse_to_f64` (LE only, `types.rs:92-132`). Whole-batch wall time is recorded once via `record_success` (590).
5. Converters: `ConverterEngine::apply_converters` maps `(id, ts, raw)` → `(id, ts, raw, converted)` on the worker thread (`converter_engine.rs:47-115`).
6. `BackendMessage::DataBatch(Vec<(u32, Duration, f64, f64)>)` is `try_send`'d; a full queue drops the batch and bumps `dropped_messages` (`worker.rs:713-718`). Stats go every 500 ms (192-196).
7. UI `process_backend_messages` drains and pushes into `VariableData` ring buffers unless paused (`frontend/mod.rs:416-429`).
8. Rate limit is `sleep(target_interval - elapsed)` after each iteration (`worker.rs:672-689`); no absolute-deadline scheduler, so period drifts by the read time. `effective_sample_rate = 1e6 / avg_batch_us` (666-670) is the read-bound ceiling, not the achieved rate. README claims "1000+ Hz" (`README.md:9`); default is 200 Hz (`config/mod.rs:65`).

`SwdCommand`/`SwdResponse` (`worker.rs:50-89`) are dead code: defined and re-exported (`lib.rs:114`, `backend/mod.rs:94`) but never sent or matched. The live protocol is `BackendCommand` (Connect/Disconnect/Start/Stop/Add/Remove/UpdateVariable/WriteVariable/ClearData/SetPollRate/RequestStats/Shutdown/RefreshProbes/SetWatchLeaves/SetWatchPollRate) and `BackendMessage` (ConnectionStatus/ConnectionError/DataPoint/DataBatch/ReadError/WriteSuccess/WriteError/Stats/VariableList/ProbeList/PointerStates/WatchValuesUpdate/Shutdown). Per-variable `Variable.poll_rate_hz` (`types.rs:395`) is never consulted by the worker; only the global rate is.

Live Watch: UI walks each root's DWARF tree every frame and sends the visible primitive/pointer leaves as `SetWatchLeaves` (`watch/mod.rs:184-221`); `WatchScheduler::tick` reads each leaf individually with `read_memory` (no coalescing), resolves `PointerDeref` children from last tick's cache (`watch_scheduler.rs:80-180`). Default 5 Hz.

## Catalog and variable discovery

- ELF: `object` symtab + dynsym → `SymbolInfo` (`elf_parser.rs:27-48`), demangled C++ (`cpp_demangle`) then Rust (`rustc-demangle`) (373-389); short display name strips templates/args (392+).
- DWARF (`dwarf_parser.rs`): single pass allocating `TypeId`s on first reference (`type_table.rs:504`), then `finish` resolves specifications, forward declarations, and inherited members (1009-1018). Handles base/pointer/reference/const/volatile/restrict/typedef/array/struct/class/union/member/inheritance/template params/enum/subroutine (433-890). `GlobalTypeKey` copes with `DW_FORM_ref_addr` from ARM compilers (`type_table.rs:53-62`). Bitfields via `DW_AT_bit_offset/bit_size` (1402-1436). Member offsets from constants or evaluated exprs (1338-1400).
- Variables: `parse_variable` (891-944) defers `DW_AT_specification`/`abstract_origin` targets. `get_variable_status` (1458-1499) plus location evaluation (1502-1845) yield 14 `VariableStatus` variants (Valid, OptimizedOut, ExternDeclaration, CompileTimeConstant, RegisterOnly, MultiPiece, Artificial…). Only `Valid`, `AddressZero`, `MultiPiece{has_address}` are readable (57-66). `DwarfDiagnostics` counts each class (129-182).
- Merge: DWARF symbols matched to ELF symbols by address first, then by name/mangled name (`elf_parser.rs:493-520`).
- Expansion: `expand_symbol_to_variables` flattens one struct level (355-369); `AppAction::AddStructVariable` carries a nested `ChildVariableSpec` tree with `Absolute` or `RelativeToPointer` addressing (`state.rs:18-66, 133-137`). Arrays capped at 1024 elements (`watch/mod.rs:19`).
- Type → plot type: `TypeTable::to_variable_type` (856) collapses to U8..F64/Bool/Raw(n).
- Re-resolution after rebuild: no file watcher. Manual `LoadElf` bumps `topics.elf_generation` and runs `detect_variable_changes` (`frontend/mod.rs:796-817, 1030-1099`): by name lookup, produces `AddressChanged`/`TypeChanged`/`NotFound`, shown in a dialog for the user to apply selectively. Live Watch roots persist only the symbol name and re-resolve on every render (`watch/mod.rs:164-178`), so they survive rebuilds for free.

## Write and tune path

`ValueEditor` dialog → `AppAction::WriteVariable{id, value: f64}` (`state.rs:147`) → `FrontendReceiver::write_variable` → `BackendCommand::WriteVariable` → `BackendWorker::write_variable` (`worker.rs:488-528`) → `DebugProbe::write_variable` → `ProbeBackend::write_variable` (`probe.rs:647-678`): converts f64 to LE bytes with `as` casts (saturating, silent), `write_8` through AP0 while the core runs.

Validation: `Variable::is_writable` = primitive type and no converter script (`types.rs:577-579`); dialog also requires `Connected` (`value_editor.rs:98`). Worker checks variable exists and connected. No range check, no readback verify, no halt, no undo, no rate limit on writes, no "tune" abstraction (no step/slider/live-drag). `WatchRow.writable` exists (`watch/mod.rs:134`) but `live_watch.rs` never issues a write. Result surfaces as `WriteSuccess`/`WriteError` and `last_error` only.

## Wire protocol

None — direct memory access. The `DebugProbe` trait (`probe_trait.rs:163-234`) is the whole surface: `connect(selector, target)`, `disconnect`, `is_connected`, `read_variable`, `read_variables` (default loops; probe-rs impl overrides with bulk), `write_variable`, `read_memory(addr, size)`, `write_memory(addr, &[u8])`, `halt`, `resume`, `reset(halt)`, `is_halted`, `stats`, `stats_mut`, `reset_stats`. Three impls: probe-rs (AP direct, non-halting), OpenOCD (TCL `mdb/mdw` per variable over TCP, `openocd/probe.rs:55-105`), mock. Backend is rebuilt on every `Connect` so config edits take effect (`worker.rs:355-376`).

## Concurrency model

- Two long-lived threads: eframe UI and `BackendWorker` spawned with `std::thread::spawn` (`main.rs:124-127`). Probe listing spawns a throwaway thread (`backend/mod.rs:229-234`).
- `tokio` is in `Cargo.toml` but has zero uses in `src/`.
- Probe is owned exclusively by the worker as `Box<dyn DebugProbe>` (`worker.rs:102`); no locks. Rhai runs on the worker; `ScriptEngine` holds `Arc<RwLock<ScriptContext>>` so `time()/dt()/prev()` builtins can read per-execution context (`engine.rs:108-118, 503-509`).
- Channels: crossbeam `bounded(256)` commands, `bounded(10_000)` messages (`backend/mod.rs:399-402`). Data/stats/pointer messages use `try_send` (drop on full); status and variable-list use blocking `send`.
- Stop: `Arc<AtomicBool>` running flag; command-channel disconnect also stops (`worker.rs:229-232`).
- Stats: `ProbeStats` keeps a 100-sample `VecDeque` of batch read times; `jitter_us = max-min`, `stddev_us`, `recent_min/max` (`probe_trait.rs:11, 108-142`). `CollectionStats` adds `dropped_messages`, `bulk_reads`, `reads_saved_by_bulk`, `effective_sample_rate`.
- `CollectionConfig.channel_buffer_size`, `.max_data_points`, `.timeout_ms` are editable in the UI but never read by the backend (constants are hardcoded).

## UI model

- egui_dock workspace; `Pane` trait with `render(&mut SharedState, &mut Ui) -> Vec<AppAction>` and `render_dialogs` (`pane_trait.rs:15-30`). Registry of 4 kinds with singleton flags and factories (`pane_registry.rs:22-49`); View menu is generated from it.
- Panes never mutate app state directly; they borrow `SharedContext` (frontend handle, ELF, display time) + `SharedMut` (config, settings, topics) and return actions (`state.rs:69-96`). `handle_action` in `frontend/mod.rs:510+` is the single dispatcher.
- Refresh: `ctx.request_repaint()` every frame while collecting, connected, or messages arrived (`frontend/mod.rs:2122-2127`) — continuous repaint whenever a probe is attached.
- Plot: `VariableData` ring buffer of 100k points (`types.rs:38, 736-763`) with incremental min/max/avg; `time_series.rs:860-877` decimates to 2000 points with a cache keyed on source length. `PlotView` (`plot.rs:33-68`) has autoscale/lock per axis, 10 s default window, 300 s max, follow-latest; `PlotCursor` A/B deltas (599-715); threshold lines and markers in `time_series.rs`; `TriggerSettings` pre/post capture in `settings.rs:201-283`.
- Converters: Rhai per variable; engine limits `set_max_operations(10_000)`, expr depth 64, call levels 32 (`engine.rs:150-155`). Script may define `fn convert(raw)` or be a bare expression over `value`/`raw` (497-542). Builtins: `derivative`, `integrate`, `smooth`, `lowpass`, `highpass`, `deadband`, `rate_limit`, `hysteresis`, math/bit ops (201-483). Compile failures drop the converter; runtime errors fall back to raw (`converter_engine.rs:84-96`). Script editor pane with 940 lines of its own UI.
- i18n: rust-i18n macro at crate root, `Language::{English, SimplifiedChinese}` (`i18n.rs:9-13`), 175-line YAML each; native `muda` menus on macOS/Windows, egui menu on Linux (`frontend/mod.rs:2129-2131`).

## Persistence

| File | Where | Contents | Anchor |
|---|---|---|---|
| `app_state.json` | `dirs::data_dir()/dev.hxyulin.datavis-rs/` | version, recent projects (max 10), last project/chip/probe, `UiPreferences` (dark mode, font scale, language) | `config/mod.rs:53-62, 162-190` |
| `*.datavisproj` | user-chosen, pretty JSON | `ProjectFile{version, name, config: AppConfig, binary_path, persistence}`; `AppConfig` = probe cfg, `HashMap<u32, Variable>`, ui cfg, collection cfg, `live_watches`, watch rate | `config/mod.rs:354-447, 555-582` |
| `ui_session.json` | app data dir, auto-saved on exit | window pos/size/maximized, serialized dock layout, toolbar/statusbar flags, last project, probe index, chip input, ELF path, and a copy of `variables` for project-less sessions | `ui_session.rs:25-66`; save at `frontend/mod.rs:2364-2382` |
| CSV/session recordings | user-chosen | `SessionRecording{metadata, frames}` from the recorder pane; `DataPersistenceConfig` streaming log capped at 2 GiB | `session/types.rs:60-149`, `config/mod.rs:460-490` |

Variable IDs are a global `AtomicU32` re-synced after load (`types.rs:585`); same pattern for `WatchId` (`watch/mod.rs:25-33`).

## Numbers

| Quantity | Value | Anchor |
|---|---|---|
| Default poll rate | 200 Hz | `config/mod.rs:65` |
| Claimed max | 1000+ Hz | `README.md:9` |
| Live Watch rate | 5 Hz default, 0 = off | `config/mod.rs:584-586` |
| Stats publish period | 500 ms | `worker.rs:192` |
| Halt sanity check | every 1 s during collection | `worker.rs:564` |
| Command channel | bounded 256 | `backend/mod.rs:399` |
| Message channel | bounded 10 000 | `backend/mod.rs:402` |
| Bulk gap threshold | 64 B | `read_manager.rs:32`, `config/mod.rs:743-745` |
| Max bulk region | 256 B (0 = unlimited) | `config/mod.rs:747-749` |
| Latency window | 100 batch samples | `probe_trait.rs:11` |
| Ring buffer per variable | 100 000 points | `types.rs:38` |
| Render decimation | 2 000 points/line | `types.rs:41` |
| Stats min/max recalc | every 1 000 evictions | `types.rs:717` |
| Plot window | 10 s default, 300 s max | `plot.rs:82-83` |
| Array expansion cap | 1 024 elements | `watch/mod.rs:19` |
| Probe speed | 4 000 kHz SWD default | `config/mod.rs:756` |
| USB timeout | 1 000 ms | `config/mod.rs:739-741` |
| Rhai ops/exec | 10 000 | `engine.rs:152` |
| Pointer validity range | 0x1000..=0xFFFF_FFFF_0000_0000, 4-aligned | `types.rs:339-341` |
| Persistence log cap | 2 GiB | `config/mod.rs:71` |
| Recent projects | 10 | `config/mod.rs:62` |
| Tests | 310+ (README), 6 integration files, 5 ELF fixtures | `README.md:101`, `tests/` |

## Reuse verdict for tuning-tools

MIT licence: everything below may be copied verbatim with attribution.

Port as-is into a Rust crate:
- `src/backend/type_table.rs` and `src/backend/dwarf_parser.rs` (~4k lines, fixture-tested): `TypeId` table, `GlobalTypeKey` for ARM `ref_addr`, `VariableStatus` classification, location-expression evaluation, template/inheritance/bitfield handling. This is the hardest part to rewrite and the most reusable. Drop the `to_variable_type` collapse to `f64` and keep the full `TypeDef`.
- `src/backend/elf_parser.rs` symbol load + DWARF merge (443-560) and `demangle_symbol` (373-389).
- `src/backend/read_manager.rs::ReadManager` (39-229): pure address coalescer with property tests; parametrise by a `MemoryRegion` newtype rather than `Variable`.
- `src/backend/probe_trait.rs::ProbeStats` rolling latency/jitter (17-148).
- `src/watch/mod.rs::walk_root` tree walk and `WatchAddress::{Static, PointerDeref}` model, plus `WatchScheduler` pointer cache (`watch_scheduler.rs`), as the basis for a Keil-style watch tree.
- `src/backend/probe.rs::arm_memory_interface` (375-383) and the non-halting AP0 read/write pattern; `write_variable` LE conversion table (653-670).
- `frontend/mod.rs:1030-1099` rebuild diff (`AddressChanged/TypeChanged/NotFound`) as the model for post-rebuild re-resolution, upgraded to run automatically on ELF mtime change.
- Rhai builtin DSP set (`engine.rs:201-483`) and safety limits (150-155) if converters are wanted in the Rust side.

Redesign:
- Transport: the `BackendCommand/BackendMessage` pair is fine as a shape, but a Tauri app needs the backend to be `Send + 'static` behind commands/events; keep a single owner thread for the probe (this design is right) but replace crossbeam `try_send`-and-drop with a bounded ring the UI pulls in bulk, and keep `dropped_messages`.
- Scheduling: replace `sleep(interval - elapsed)` with an absolute-deadline loop; stamp each batch with both host `Instant` and a target-side tick when firmware exposes one. The `effective_sample_rate` formula (`worker.rs:666-670`) misreports; report achieved rate from batch timestamps.
- Coalescing should be planned once per variable-set change, not per poll (`probe.rs:485-490` rebuilds `ReadManager` and re-sorts every poll).
- Write path: add typed range validation from `TypeDef` (enums, bitfields, integer bounds), optional readback, and a tune widget layer (drag/step/apply-on-release). Live Watch write is unimplemented; do it at the tree-row level from day one.
- Per-variable rates: the `Variable.poll_rate_hz` field exists but is ignored by the worker; implement rate groups in the planner or drop the field.
- Rebuild handling: add a file watcher; keep symbol-name (not address) as the persisted identity, as Live Watch already does.
- Config plumbing: `channel_buffer_size`, `max_data_points`, `timeout_ms` are UI-editable but unused; either wire them or delete them.

Drop:
- All egui/egui_dock/eframe frontend (`src/frontend/*`, `src/menu/*`, 10k+ lines): Tauri replaces it. Keep only the `AppAction` list as a checklist of required commands.
- `SwdCommand/SwdResponse` (dead), the `tokio` dependency (unused), `mock_fault.rs`/`mock_probe.rs` unless a simulated target is wanted for UI dev (then keep `MockDataPattern` only).
- OpenOCD backend (`src/backend/openocd/*`, ~1.2k lines) unless J-Link-less targets require it; it has no bulk reads and per-variable TCP round trips.
- `ui_session.json` variable auto-save and `.datavisproj` JSON shape; tuning-tools should persist by symbol path + ELF hash, not by `u32` id + address.
- Rhai converters on the read path if tuning-tools keeps raw typed values and does transforms in the UI; converters also block writes (`types.rs:578`), which is the wrong coupling for a tune tool.
