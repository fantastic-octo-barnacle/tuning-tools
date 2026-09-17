# MemRW3

Source: https://github.com/SuperLiaohy/MemRW3 (SuperLiaohy). Licence: **no `LICENSE*`/`COPYING*` file in the repo and no `license` field in `Cargo.toml`; the only statement is `README.md:265-267` (`## 许可证` -> `MIT`).** Treat as unlicensed for code reuse until a licence file lands. Stack: Rust 2024 (rust-version 1.85), eframe/egui 0.34 + egui_plot 0.35 + egui_ltreeview 0.7, probe-rs `=0.31.0` + probe-rs-debug `=0.31.0`, gimli 0.31 + object 0.36 (own DWARF walker), capstone 0.14, svd-parser 0.14, crossbeam-queue, rfd, fontdb. Size: 35 `.rs` files, 19,390 lines, last commit `c47c294` 2026-09-07. Author's write-up: bbs.herkules.dev article `01M2547P1CHPAVFX4N5JND7CYQ`. Paths below are relative to the clone at `scratchpad/memrw3/`.

## Purpose

Desktop "memory read/write monitor" for Cortex-M/RISC-V targets over a debug probe: pick globals/struct fields/array elements from an ELF's DWARF, poll them through the probe's MEM-AP while the firmware runs (no firmware cooperation), plot/FFT/log them, write them back, browse SVD peripheral registers, flash firmware, and (since mid-2026) run a source-level debugger (hardware breakpoints, step into/over/out, locals, call stack) inside the same session. Single probe, core 0 only, one hardware-owner thread.

## Module map

| Path | Responsibility |
|---|---|
| `src/main.rs` | eframe bootstrap, 1280x720 window, CJK font install (`:14-27`) |
| `src/app.rs` | `MemRW3App`: owns session state, `Vec<Box<dyn MemRWPlugin>>`, worker handle, toasts; slot rebuild (`:257`), plugin action dispatch (`:320`), worker event poll (`:476`), per-frame `update` (`:649-791`), config save/load (`:809-985`), font discovery (`:988-1110`) |
| `src/model/state.rs` | `AppSession` (shared atomics: `acquisition_requested`, `running`, `acq_cycle_count`, `slot_count`; `sampling_hz`), `Config` (`delay_us: Arc<AtomicU64>`, chip/protocol/speed defaults `:63-76`) |
| `src/model/variable_pool.rs` | `VariablePool`: id-indexed vec of `PooledVariable{incoming: Arc<RingBuffer>, latest: Arc<LatestValue>, stream_readers, latest_readers}`; `bind/unbind` refcounts per read class (`:133-206`) |
| `src/model/ring_buffer.rs` | crossbeam `ArrayQueue` wrapper, capacity 2560, `force_push` drops oldest (`:3,:33-35`), `drain_into` (`:41`) |
| `src/model/debug.rs` | `DebugCommand`, `DebugSnapshot` (revision/program_generation/stop_id), `BreakpointSpec`, `StepKind`, `StepExecutionMethod` |
| `src/model/register_io.rs` | SVD register read/write request + result types (`RegisterData = HashMap<id, RegisterReadResult>`) |
| `src/probe/session.rs` | `ProbeSession`: connect/attach, cached `Core<'static>` (`:96-115`), slot merge + one read cycle (`:312-401`), flash (`:248-308`), write/register IO (`:415-466`) |
| `src/probe/worker.rs` | `ProbeWorker` thread: command loop (`:303-403`), acquisition scheduling, `DebugEngine` (start/halt/continue/step/run-to/breakpoints/unwind/disassembly `:709-2870`), link health |
| `src/dwarf/extract.rs` | ELF+DWARF walk via gimli: CUs, static variables, type resolution, struct/array/pointer nodes (`:231-1278`); line-table index for the debugger (`:56-156`) |
| `src/dwarf/types.rs` | `TreeNode`, `BasicType`/`ExtendType`, `DwarfState` search, array path re-track (`:229-447`) |
| `src/svd/mod.rs` | svd-parser with `expand(true)`: arrays/clusters/derivedFrom flattened to absolute-address registers + fields |
| `src/ui/plugin.rs` | `MemRWPlugin` trait, `PluginAction`, `PluginUpdateContext`/`PluginRenderContext`, `FrameData` |
| `src/ui/dock.rs` | activity bar, active plugin pane, pop-out native viewports, per-plugin pause |
| `src/ui/control_bar.rs` | connect / start-pause / probe settings modal / delay slider / reset / flash / save / load / theme / Hz status |
| `src/ui/variable_tree_panel.rs` | "VariTree": bottom-sheet or pop-out ELF browser shared by all plugins; materialises `VariableCandidate` trees; `追踪` re-track |
| `src/ui/vari_tree.rs`, `vari_properties.rs` | egui_ltreeview DWARF tree with search; Basic/Extend/Add property editor |
| `src/ui/chart_plugin/{panel,legend,fft,line_dialog}.rs` | Chart plugin: time-domain plot, FFT pane, legend overlay, CSV log |
| `src/ui/table_plugin/{panel,tree,svd_panel}.rs` | Table plugin: variable tree with per-leaf refresh + write; SVD register pane |
| `src/ui/debug_plugin/{mod,workspace}.rs` | Debug plugin: 5-pane IDE layout, source/asm/split views, breakpoints, locals, registers, stack memory |
| `src/ui/theme.rs`, `native_dialog.rs` | light/dark palettes, Linux gsettings theme poller; rfd dialogs parented to main window |

Plugin architecture: `MemRWPlugin` (`src/ui/plugin.rs:110-158`) = `id/title/update/render/add_variable_ui/save_config/load_config/on_enabled_changed/reset_data`. Plugins never touch hardware; they return `PluginAction`s (`:55-90`) which `App::handle_plugin_actions` turns into pool mutations and `ProbeCommand`s. Built-ins: Chart (`"chart"`, stream reader), Table (`"table"`, latest reader, composite variables), Debug (`"debug"`). VariTree is not a plugin: it is a shared panel targeted at the plugin that opened it (`OpenVariableTree{plugin_id, viewport_id}`) and calls that plugin's `add_variable_ui`.

```
 egui main thread                                   ProbeWorker thread (sole hw owner)
 ┌──────────────────────────────────────────┐        ┌──────────────────────────────────┐
 │ control_bar  dock  VariTree(DWARF)        │  mpsc  │ ProbeCommand loop                 │
 │ Chart | Table+SVD | Debug  (MemRWPlugin)  │──cmd──▶│  ProbeSession ── probe-rs Session │
 │      ▲ PluginAction        ▲ FrameData    │◀─evt───│   Core<'static> (MEM-AP reads)    │
 │      │                     │ RegisterData │        │  DebugEngine (DebugInfo, FPB)     │
 │ MemRW3App ── VariablePool ─┤ DebugSnapshot│ atomics│  cycle_count, running, delay_us   │
 │              (RingBuffer + LatestValue)   │◀──────▶│                                   │
 └──────────────────────────────────────────┘        └──────────────┬───────────────────┘
                                                        USB probe ─ SWD/JTAG ─ DAP ─ MEM-AP ─ AHB ─ SRAM/peripherals
```

## Acquisition path

Chain (article + `session.rs`): host `MemRW3 -> USB probe (CMSIS-DAP / ST-Link / J-Link) -> SWD/JTAG -> MEM-AP -> SRAM`. The core is never halted for sampling; reads are background AHB-AP transactions.

Slot merge (`app.rs:257-306`, `session.rs:312-322`): every enabled pool variable (size capped at 8) is expanded to 32-bit aligned word addresses `start = addr & !3, step 4`; a `HashMap<u64, slot_idx>` dedups, so two variables sharing a word cost one `read_word_32`. Each `VarSlotMapping{slot_indices, size, byte_offset, incoming, latest, stream_enabled, latest_enabled}` is sent to the worker with `ProbeCommand::ConfigureSlots`; `AcqSlot.needed_for_latest` marks slots that must still be read when only Table readers exist.

Read cycle (`session.rs:335-401`): `ts = timer.elapsed()` (host `Instant`, reset by `ProbeCommand::ResetTimer`, one timestamp per cycle shared by all variables); for each slot `core.read_word_32(addr)` into `slot_values[i]`; then per variable the bytes are re-assembled from its slots at `byte_offset`, pushed as `(ts, [u8;8])` to `incoming` when a stream reader exists and stored into `latest` (AtomicU64 + sequence) when a latest reader exists. So one SWD word read feeds both Chart (stream) and Table (latest). Core handle reuse: `ensure_core` caches `Core<'static>` via `transmute` of `session.core(0)` with the `Session` boxed to pin its address (`session.rs:96-115`); invalidated before flashing or any exclusive `Session` use.

Scheduling (`worker.rs:303-387`): drain up to 64 commands; if debug is active poll target status every 50 ms; `acquisition_running = running && target in {Running, Sleeping, Unknown}` -> `acquire_from_slots_mode(true)`, `cycle_count += 1`, then `sleep(delay_us)` if non-zero else `recv_timeout(20 ms)` only when idle. `delay_us == 0` is "full speed" (busy loop bounded by probe latency); the slider is 0..=10000 us step 50 (`control_bar.rs:401-419`). Halted target -> latest-only reads once per second (`worker.rs:339-345`), Chart stops. Link health: `core.status()` every 500 ms when idle or after a failed cycle; 3 consecutive failures -> disconnect + `LinkLost` (`worker.rs:247-249, 1772-1790`).

Consumption: `App::ui` drains every ring buffer into `FrameData` each frame (`app.rs:671-681`), computes `sampling_hz = delta cycles / delta t` once per second (`:663-669`), then calls `plugin.update`. Chart pushes samples into per-legend `VecDeque<PlotPoint>` (`chart_plugin/panel.rs:496-510`); Table reads `latest` and falls back to the last frame sample (`table_plugin/tree.rs:236-241`).

Measured rate (article): **>7 kHz cycles/s for a single variable at delay 0, Linux host, wired CMSIS-DAPv2 DAPLink, SWD 10 MHz**. Wired bottleneck = SWD IO + polling latency; wireless probes need a raised delay. Multi-variable rate scales inversely with slot count since every slot is a separate `read_word_32`.

## Catalog and variable discovery

ELF loaded with `object` + `gimli` (`dwarf/extract.rs:11-44, 195-229`). `collect_cus` (`:231-390`) walks every CU: pre-pass indexes named type definitions so declarations resolve cross-CU; namespaces prefix names `ns::name`; only `DW_TAG_variable` with a resolvable `DW_OP_addr/addrx` location (`:1010-1048`) becomes a root `TreeNode{name, type_name, basic_type, address, size, children}`. Struct/union/class fields (`DW_TAG_member`, plus flattened `DW_TAG_inheritance` with base offset `:1075-1187`) become children whose `address` is an offset relative to the parent; typedef/const/volatile pass through; pointers become `T *` with `size = address_size` (`:604-997`). Arrays produce a single `[0]` prototype child and `BasicType::ArrayElem(elem, count)` (`:442-509`); the UI edits the index and computes `addr = base + idx * elem_size` (`vari_properties.rs:41-58`, `types.rs:229-260`). `type_name_to_basic_type` (`:1222-1278`) maps C names to `U8..I64/Float/Double` with a size fallback; `char -> I8`, `bool -> U8`.

Property editor (`vari_properties.rs`): **Basic** = read-only DWARF facts (name, offset, size, type); **Extend** = editable name/absolute address/`ExtendType` (u8..u64, i8..i64, float, double, other) that the plugin actually samples; **Add** delegates to the plugin's `add_variable_ui`. Composite (`Other`) roots are only accepted by plugins with `supports_composite_variables` (Table) and are materialised recursively, arrays expanded to every element (`variable_tree_panel.rs:562-644`).

Re-track after rebuild (`追踪`, `variable_tree_panel.rs:440-463, 493-518`): reload the ELF, then for each pooled variable `expand_bracket_path("a.b[3].c")` and `trace_exact` by full path (`types.rs:351-368, 553-578`); exactly one match updates address/type/size, zero or many raise a toast; then `RebuildSlots`. Config load runs the same `trace_pool` before accepting the file (`prepare_config`, `:414-428`).

SVD (`svd/mod.rs`, `table_plugin/svd_panel.rs`): parsed in a spawned thread; peripherals sorted by base address; registers get absolute addresses, size bits, access, reset value, fields. Search matches top-level peripheral names only.

## Write and tune path

Table leaf: text box + `写` -> `validate_write` per `ExtendType` (`table_plugin/panel.rs:446-491`): integers parsed as the exact Rust type (so `u8` 0..255, `i16` -32768..32767 ...), floats via `f32/f64::to_bits`, `Other` rejected. `PluginAction::WriteVariable{var_id, value:u64}` -> `ProbeCommand::WriteValue{request_id, address, size, value}` -> `write_word_8/16/32/64` by size (`session.rs:415-429`) on the same core handle between acquisition cycles; result comes back as a `WriteResult` event/toast. Debug locals: `Variable::update_value` through probe-rs-debug, scalars at `VariableLocation::Address` only (`worker.rs:1374-1433`).

Caveats stated by the author: the write is a raw memory store racing the firmware, so avoid tuning variables that an ISR or another task rewrites every cycle (the write is lost or torn); `HAL_Delay`/SysTick-driven code misbehaves while PRIMASK is masked during source steps. SVD register writes go straight to the peripheral bus and honour only the SVD `access` string (`svd_panel.rs:556-565`); reading a register can itself have side effects (read-to-clear status, write-1-to-clear, FIFO pop), which the tool warns about but cannot prevent. Write values are parsed as hex (`0x`) or decimal and range-checked to the register byte width (`:593-615`).

## Wire protocol

None. The target is passive; MemRW3 talks only to the probe through probe-rs. API surface used: `Lister::list_all`, `Probe::open/select_protocol/set_speed/attach` or `Session::auto_attach` (`session.rs:138-229`); `MemoryInterface::{read_word_32, read, write_word_8/16/32/64}`; `Core::{status, halt(200 ms), run, step, reset, reset_and_halt(500 ms), spill_registers, available_breakpoint_units, set_hw_breakpoint, clear_hw_breakpoint, read_core_reg, write_core_reg}`; `probe_rs::flashing::download_file_with_options` with `verify = true` and format from extension (`session.rs:248-308`); `probe_rs::config::Registry::from_builtin_families` for the chip list; probe-rs-debug `DebugInfo::{from_file, get_source_location, get_breakpoint_location, unwind}`, `DebugRegisters::from_core`, `VariableCache`.

Debug path (worker.rs): source breakpoint = `BreakpointSpec::Source{path,line}` -> path normalised and matched by exact then longest-unique-suffix against the DWARF file list (`:1826-1885`) -> `get_breakpoint_location`, fallback to nearest `is_stmt` line within 32 (`:1887-1903`) -> one FPB comparator via `set_hw_breakpoint` (`:1464-1526`); user breakpoints and temporary ones share the comparator pool (`breakpoint_capacity` from `available_breakpoint_units`). `Step Instruction` = `core.step()`. Source steps (`single_step_source`, `:1931-2094`): record origin line + unwind depth; set PRIMASK by writing bit 0 of the `EXTRA`/`EXTRA_S` register and reading it back, abort the step if it cannot be verified (`:2268-2299`); `Into` = repeated hardware single-steps until the line changes; `Over` on a byte-verified `bl/blx/blr` = restore mask, temp hw breakpoint at the fall-through address and run (`:2115-2127, 2164-2223`); `Out` = temp breakpoint at the unwound caller resume address (Thumb2/RV32C +2, RV32 +4, Xtensa +3; `:2129-2137`). If no comparator is free, `run_to_step_target` returns `None` and the engine falls back to continuous single-step, reporting `StepExecutionMethod::SingleStep` vs `HardwareBreakpoint` in the snapshot (shown in the toolbar, `debug_plugin/mod.rs:697-703`). Limits: 16384 instructions or 10 s per source step (`:36-37`); an `AtomicU8` interrupt flag (`HALT<STOP<RESET<SHUTDOWN`, `fetch_max`, `:30-34, 158-183`) lets the UI abort a running step. Halt snapshot (`:1218-1313`): spilled registers, up to 64 unwound frames, 32 stack words within RAM, disassembly window from the ELF (64 B before / 512 B after PC, <=160 instr) or live memory (<=128 B). NMI/HardFault are not maskable; only C locals are supported (C++ mapped to `DW_LANG_C` temporarily).

## Concurrency model

Threads: (1) egui main thread; (2) `ProbeWorker` spawned in `ProbeWorkerHandle::spawn` (`worker.rs:124-156`), the sole owner of `Probe`, `Session`, cached `Core`, `DebugInfo` (built inside the worker because it is `!Send`), breakpoints and program code; (3) transient SVD parse thread (`svd_panel.rs:76-89`); (4) Linux-only theme poller (`theme.rs:78-98`). Channels: unbounded std `mpsc` for `ProbeCommand` (UI->worker, drained <=64 per loop) and `ProbeEvent` (worker->UI, polled each frame in `poll_probe_events`). Shared atomics: `acquisition_requested`, `running`, `delay_us`, `acq_cycle_count`, `slot_count`, `step_interrupt`. Data: per-variable crossbeam `ArrayQueue<(f64,[u8;8])>` (worker pushes, UI drains) and `LatestValue{AtomicU64 value, AtomicU64 sequence}`; `DebugSnapshot` is cloned into an event, accepted only if `program_generation` matches and `revision` is not older (`app.rs:618-622`); `stop_id` guards `ExpandVariable/WriteVariable` against stale halts. Repaint: `ctx.request_repaint()` every frame while running (`app.rs:658-661`), `ProbeWorker::emit` requests a repaint after every event (`worker.rs:1792-1795`) and after halted latest reads, `request_repaint_after(50 ms)` while flashing or parsing SVD, SVD scheduling asks for a repaint at the next due read. Dropping the handle sends `Shutdown` and joins (`worker.rs:195-199`).

## UI model

egui/eframe, single window plus native pop-out viewports (`show_viewport_immediate`, `dock.rs:348-407`). Layout: control bar on top; 52 px activity bar (`app.rs:712`) with one 44x42 icon button per plugin; the active plugin fills the centre; each plugin has a dock control bar with `Pop out`/`Pop in` and `暂停插件` (pause stops `update` and interaction, keeps the picture; Debug reacts with `DebugCommand::Stop`). VariTree opens as a bottom sheet (30-80 % of the host viewport, draggable handle) over the plugin that requested it or as its own 1000x620 viewport (`variable_tree_panel.rs:69-200`).

Chart (`chart_plugin/panel.rs`): multi-curve `egui_plot` lines from `VecDeque` slices (wrap bridged, `legend.rs:59-66`); per-curve buffer 5000 default, 1000..=50000 (`legend.rs:24`, `line_dialog.rs:38`); X auto window max(span, 6 s) or fixed seconds, Y auto/fixed/hidden; wheel zoom with X/Y/Both mode buttons, step 1.02, drag pans and leaves auto-scroll, double-click returns; cursor vline with per-curve nearest-sample overlay; optional sample markers when <= threshold (2..1000, default 32) points are visible; legend overlay top-right: left-click toggles visibility, right-click opens the edit modal (name, colour with 12 presets, buffer, visible, delete) (`:1546-1603`). FFT pane (`fft.rs`): toggle splits the area 55 % time / 45 % FFT; self-contained radix-2, windows Rectangular/Hann/Hamming/Blackman, N = 4..65536 points taken from the tail, latest contiguous segment only (gap > 3x median interval starts a new segment), linear resample to a uniform grid, coherent-gain-corrected one-sided magnitude, result cached until the input window changes bit-for-bit. CSV log: choose a file before starting; the file is recreated at acquisition start with header `timestamp,<curve>...`, one row per distinct timestamp per frame (blank cells for curves lacking that timestamp), flushed every frame, closed when acquisition pauses (`:474-538, 852-898`). Chart-side `acq_hz` counts samples per second.

Table (`table_plugin/panel.rs`, `tree.rs`): left pane tree of roots (scalar, struct, array) with tri-state group checkboxes, per-leaf enable, value as `0x.. (dec)` or float, per-leaf display refresh 1..=60 Hz (default 10) that throttles only the text update, write box + `写`, root delete. Right pane SVD: load file, filter, collapse all, per-register checkbox + 1..30 Hz rate (default 5), value, write row when writable, fields with bit ranges; due reads are batched into one `ReadRegisters` per frame and skipped while `hardware_busy` (`svd_panel.rs:130-193`).

Debug (`debug_plugin/mod.rs`): toolbar row 1 = status dot, PC, step method, error/warning/halt reason; row 2 = Attach/Reset start mode, Start, Stop, Pause (F6), Continue (F5), Run-to-cursor, Instruction step, Step Into (F11), Over (F10), Out (Shift+F11), Interrupt, Refresh; F9 toggles a breakpoint at the cursor (`:2628-2667`). Panes (`workspace.rs`): navigator (project tree with common-root compression and filter | breakpoints with `used/capacity`), editor (Assembly | Source tabs | Split), inspector (call stack | stack memory), bottom locals (expandable, editable) and registers. Source view is virtualised (`show_rows`), shows executable-line dots, breakpoints, PC, inline per-line disassembly expanders, and maps DWARF paths to a local root override (`:2939-2964`).

Config save/load buttons live in the control bar; load is disabled while connected/running/flashing. Theme button cycles Dark -> Light -> System (`theme.rs:56-62`); light is the default. CJK font discovery: `fontdb` scans system fonts, keeps faces that contain ten probe glyphs (`app.rs:1042`), ranks by per-OS family preference lists, normal style, weight distance and monospace, and installs the winner as a fallback in both egui families (`:988-1110`).

## Persistence

Single JSON file via rfd (default `memrw3_config.json`, `app.rs:827-880`): `{elf_path, probe_chip, probe_protocol, probe_speed_khz, variables:[{name,address,ext_type,size}], plugins:[{plugin_id, payload}]}` (`:809-824`). Payloads: Chart `{legends:[{variable_name, variable_address, variable_type, variable_size, curve_name, color[4], visible, buffer_size}], show_sparse_points, sparse_point_threshold}` (`chart_plugin/panel.rs:172-193`); Table `{roots:[SavedTableNode{label, source_name, source_address, expanded, variable?, children}], svd_path, svd_registers:[{peripheral_name, register_name, address, enabled, read_hz}]}` (`table_plugin/panel.rs:44-51`, `tree.rs:271-295`); Debug `{breakpoints:[LogicalBreakpoint{id, spec, enabled}], source_root_override, workspace_layout, start_mode}` (`debug_plugin/mod.rs:133-142`). Legacy flat array payloads still deserialise (`#[serde(untagged)]`). Loading rebuilds the pool, re-traces every variable against the ELF, then feeds each plugin its payload; breakpoints are re-sent after the next program load. Not persisted: delay_us, probe serial, theme, window/dock layout, chart history (eframe persistence is unused). Hardware breakpoint comparators are never saved, only logical specs.

## Numbers

| Item | Value | Anchor |
|---|---|---|
| Single-variable full-speed rate | >7 kHz (Linux, DAPLink CMSIS-DAPv2 wired, SWD 10 MHz) | article |
| Read granularity | 32-bit word per slot, variable size <= 8 B | `session.rs:312-322` |
| Delay slider | 0..=10000 us, step 50; 0 = full speed | `control_bar.rs:407-409` |
| Probe speed | 100..=20000 kHz, default 10000; default chip STM32F407VG / SWD | `control_bar.rs:228`, `state.rs:74-76` |
| Command drain per loop | 64 | `worker.rs:389-403` |
| Idle wait / status poll / link check | 20 ms / 50 ms / 500 ms | `worker.rs:247-249, 323-329` |
| Link failures before disconnect | 3 | `worker.rs:249` |
| Halted latest-read period | 1 s | `worker.rs:339-345` |
| Per-variable ring buffer | 2560 samples, drop-oldest | `ring_buffer.rs:3, 33-35` |
| Chart buffer per curve | 5000 default, 1000..=50000 | `legend.rs:24`, `line_dialog.rs:38` |
| Chart X window minimum | 6 s | `panel.rs:44` |
| Wheel zoom step | 1.02 | `panel.rs:20` |
| Sample marker threshold | 2..1000, default 32 | `panel.rs:21-23` |
| FFT points | 4..65536, default 1024, Hann; gap factor 3, baseline 3 intervals | `fft.rs:171-173`, `panel.rs:128-129` |
| FFT/time split | 45 % / 55 % | `panel.rs:753` |
| Table display refresh | 1..=60 Hz, default 10 | `tree.rs:305`, `panel.rs:389` |
| SVD register read | 1..=30 Hz, default 5; 1..64-bit registers | `svd_panel.rs:15-17, 567-575` |
| Source step limits | 16384 instructions or 10 s | `worker.rs:36-37` |
| Breakpoint line fallback | nearest is_stmt line within 32 | `worker.rs:1887-1903` |
| Unwind depth | 64 frames (snapshot), 32 (step depth check) | `worker.rs:1218-1313, 2247-2266` |
| Stack memory dump | 32 words, must lie in RAM | `worker.rs:2576-2613` |
| Disassembly window | ELF: 64 B before, 512 B, <=160 instr; live: <=128 B | `worker.rs:2615-2794` |
| Halt / reset timeouts | `halt(200 ms)`, `reset_and_halt(500 ms)` | `worker.rs:709-809` |
| Windows | main 1280x720 (min 800x500); Chart 720x420; Table 1080x620; Debug 1160x720; VariTree 1000x620 | `main.rs:20-21`, plugin `viewport_size` |
| Theme poll (Linux) | every 2 s via gsettings | `theme.rs:90-95` |

## Reuse verdict for tuning-tools

Licence caveat first: the repo ships no licence file and no Cargo `license` metadata; the README's one-word "MIT" is not a grant we can rely on. **Do not copy code**; adopt designs and re-implement.

Adopt:
- The one-hardware-owner thread with a command channel, event channel and a handful of shared atomics; every UI feature expressed as a `PluginAction` that the app translates into commands. It keeps probe/serial ownership trivial and makes the UI layer testable without hardware.
- Deduplicated aligned read slots + `VarSlotMapping` with `byte_offset`, and the Stream vs Latest read-class split (ring buffer for plots, atomic latest for tables) so one bus read feeds every consumer.
- `program_generation` / `revision` / `stop_id` staleness guards on every asynchronous result; re-track variables by full source path (`a.b[3].c`) after a rebuild instead of by address.
- DWARF catalog shape: root globals, struct fields as relative offsets, arrays as one prototype child + count, Basic (DWARF truth) vs Extend (what is actually sampled) split, composite roots materialised recursively.
- Chart details worth keeping: per-curve bounded `VecDeque` with batch eviction, FFT on the latest contiguous segment with uniform resampling and coherent-gain correction, FFT cache keyed on the input window, CSV rows merged by timestamp with blank cells, display refresh rate decoupled from acquisition rate.
- SVD handling: expand arrays/clusters/derivedFrom up front, gate read/write on `access`, batch due register reads per frame, warn about read-side-effect registers.
- Debug engine tricks if tuning-tools ever grows a debugger: PRIMASK masking with read-back verification, temp hardware breakpoint for step-over/out with single-step fallback, longest-unique-suffix source path matching, lexical path canonicalisation for Windows toolchains.

Do differently:
- Tauri/web UI: the "plugin returns actions, app owns state" boundary maps onto Tauri commands + events, but the per-frame drain of ring buffers into `FrameData` does not; batch samples into fixed-interval frames on the Rust side and push them to the webview as typed arrays, keep the plot history in Rust (or a worker) and send decimated views, never raw >7 kHz samples over IPC.
- MemRW3 gets away with host-side timestamps because the probe path has no target clock. tuning-tools needs a firmware-cooperative protocol as well (UART/USB/CAN streaming, firmware-side timestamps and sequence numbers, explicit set/get with acks) so writes are atomic on the target, ISR-owned variables can be tuned safely and rates are not bounded by SWD polling. Keep the probe/MEM-AP path as a second transport behind the same slot/variable abstraction rather than the only one.
- Persist more than MemRW3 does (delay/rate, probe serial, layout, theme) and version the config schema instead of relying on untagged serde enums for legacy files.
- Fonts/theme/native dialogs: none of the egui-specific work (fontdb CJK discovery, gsettings poller, `Core<'static>` transmute) is needed; the transmute in particular should be replaced by a plain owned-session design with explicit core borrowing per operation.
- Multi-core, RTT and Rust-firmware DWARF (probe-rs-debug already handles Rust locals; the static-variable walker would need Rust name mangling and enum layouts) are all open in MemRW3 and should be designed in from the start.
