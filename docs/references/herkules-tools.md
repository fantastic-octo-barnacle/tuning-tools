# herkules-tools

Source: `/Users/hxyulin/dev/RM/Software/herkules-tools`. No `LICENSE` at repo root (`package.json` is `private`); only the two Rust crates declare `license = "MIT"` (`crates/herkules-dwarf/Cargo.toml:6`).
Stack: Vite 8 + React 18 + TypeScript 5.6 + Tailwind 3 + uPlot 1.6 + react-router 7 (`package.json`); Tauri 2 host with probe-rs 0.31, serialport 4, rfd 0.15, no tokio (`src-tauri/Cargo.toml:10-18`); Cargo workspace of three crates (`Cargo.toml:3-7`); gimli 0.32 + object 0.38 for DWARF; wasm-bindgen pinned `=0.2.126`. Hosted build is static on Cloudflare assets (`wrangler.toml`).
History: 7 commits, 3-4 Aug 2026, then idle. `f169721` scaffold + tested signal core; `1a76de7` Data Explorer UI, MD2 port, app shell; `f3470ee` README; `3097374` sample CSV; `ee6a64b` six more filters; `5e99bd0` live streaming + Tauri companion (serial, probe-rs, RTT, OpenOCD, recording, mock stream); `1d371c2` shared DWARF crate + WASM web backend.
Size (lines, excluding tests/fixtures): `src/core` 4840 (filters ~2100, csv 339, pipeline 276, streaming ~750), `src/tools/dataexplorer` 2310 (808 of it is `sampleData.ts`), `src/sources` 571, `src/components` 404, `src/workers` 81, `src-tauri/src` 1465, `crates/herkules-dwarf/src` 4195 (`dwarf_parser.rs` 2319, `type_table.rs` 1629), `crates/herkules-dwarf-wasm/src` 10. TS tests 1771 lines; Rust has unit tests in `text_decoder.rs`, `tcl_client.rs`, `controller.rs`, `elf.rs`.

## Purpose

Browser-first "instrument" for RoboMaster logs: load a CSV (or a live text stream), push one channel through a linear chain of DSP filters, plot raw vs. filtered stages on an oscilloscope-styled uPlot, record synchronized CSV. The Tauri build adds native serial, direct probe-rs, RTT, DWARF-driven target-memory polling and OpenOCD, all read-only (`docs/desktop-development.md:20`). A second unrelated tool (Markdown to WangEditor) shares the shell (`src/config/tools.ts`).

## Module map

| Path | Responsibility |
|---|---|
| `src/core/series.ts` | `Series {name,time,values}` as Float64Array; `meanDt`, `timingJitter`, `extent` (`:10-104`) |
| `src/core/csv.ts` | offline CSV importer: delimiter/time-column/unit detection, `parseCell` gap policy (`:53-104`), `parseCsv` (`:218`) |
| `src/core/filters/*` | 15 `FilterDef`s with param schemas (`types.ts:54-67`), registry `FILTERS` (`index.ts:28-49`), NaN policy comment (`index.ts:11-26`) |
| `src/core/pipeline.ts` | node list stored as DAG, Kahn topo sort, memoised evaluation (`:85-224`), linear-chain helpers (`:227-276`) |
| `src/core/persist.ts` | `PersistedState v1` to URL hash (base64url) and localStorage (`:12-118`) |
| `src/core/streaming/types.ts` | `SampleBatch`, `LiveSource`/`LiveConnection`, `StreamDecoder`, `SourceStatus` (`:17-69`) |
| `src/core/streaming/decoders/{ndjson,delimited}.ts` | protocol v1 decoders (browser side) |
| `src/core/streaming/ringBuffer.ts` | `MultiChannelRingBuffer` (NaN-filled per-channel arrays) + `snapshotLatest` |
| `src/core/streaming/decimation.ts` | min/max bucket decimation for plotting |
| `src/core/streaming/{recorder,browserRecorder}.ts` | CSV row formatting; File System Access writer with Blob fallback |
| `src/sources/websocket.ts`, `browserSerial.ts` | browser transports with reconnect / WebSerial reader loop |
| `src/sources/nativeBridge.ts`, `nativeQueue.ts` | `invoke` shim over `window.__TAURI_INTERNALS__` (`nativeBridge.ts:13-18`); 25 ms pull loop over the native batch queue |
| `src/sources/streamWorkerClient.ts`, `src/workers/stream.worker.ts` | ring buffer lives in a Web Worker; 30 Hz snapshot publish |
| `src/workers/dwarf.worker.ts`, `src/backends/dwarf/*` | WASM DWARF parse off-main-thread; shared TS DTOs |
| `src/components/chart/Plot.tsx`, `ChartStack.tsx` | uPlot wrapper (setData fast path), overlay/stacked modes with cursor sync |
| `src/tools/dataexplorer/*` | page, `useDataExplorer` (pipeline state), `useLiveStream` (transport state), source chooser + panels |
| `crates/herkules-dwarf` | `parse_elf(bytes, path) -> ElfParseDto` (`src/elf.rs:31`), `DwarfParser` (`src/dwarf_parser.rs:272`), `TypeTable` |
| `crates/herkules-dwarf-wasm` | one `parse_elf_json` export (`src/lib.rs:4-10`) |
| `src-tauri/src/main.rs` | 22 `#[tauri::command]`s, all synchronous (`:292-315`) |
| `src-tauri/src/app_state.rs` | four mutexes: serial, debug, batch queue, recorder (`:8-13`) |
| `src-tauri/src/dto.rs` | camelCase serde DTOs incl. `NativeSampleBatch` (`:53-61`), `DebugVariableDto` (`:39-47`) |
| `src-tauri/src/text_decoder.rs` | native NDJSON/CSV line decoder used by serial and RTT |
| `src-tauri/src/serial/mod.rs` | `SerialController` thread + `push_bounded` queue helper (`:105-112`) |
| `src-tauri/src/debug/controller.rs` | `DebugTransport` enum (probe-rs Session or OpenOCD TCL) (`:17-26`), polling and RTT worker threads |
| `src-tauri/src/debug/openocd/{process,tcl_client}.rs` | spawn bundled OpenOCD; minimal TCL RPC client |
| `src-tauri/src/debug/parser.rs` | rfd file picker + `herkules_dwarf::parse_elf` (16 lines) |
| `src-tauri/src/recording/mod.rs` | `NativeRecorder`: BufWriter to `<path>.partial`, rename on finish |

```
            React UI (tools/dataexplorer)  ──  core/ (pure TS: series, filters, pipeline, persist)
                 │ postMessage                          │ postMessage
   ┌─────────────┴──────────────┐            ┌──────────┴───────────┐
   │ stream.worker (ring 60k)   │            │ dwarf.worker (WASM)  │  ← hosted web only path
   └────────────────────────────┘            └──────────────────────┘
   sources/: websocket.ts  browserSerial.ts  │  nativeQueue.ts ── invoke() ── Tauri IPC (JSON)
                                                                              │
                                              src-tauri: commands → AppState{serial,debug,batches,recorder}
                                                        std::thread workers → VecDeque<NativeSampleBatch>
                                                        probe-rs Session | OpenOCD child + TCL TCP | serialport
                                              crates/herkules-dwarf (native) ── same crate ── herkules-dwarf-wasm
```

## Acquisition path

Carriers and where bytes become batches:

| Carrier | Where decoded | Notes |
|---|---|---|
| WebSocket text frames | `NdjsonDecoder`/`DelimitedStreamDecoder` in page thread (`websocket.ts:106-114`) | binary frames ignored (`:102`); reconnect backoff 0.5/1/2/5/10 s (`:140-143`), decoder `reset(true)` marks discontinuity |
| WebSerial | same decoders via `ReadableStream` reader (`browserSerial.ts:112-132`) | baud 300..4 000 000 (`:52`) |
| Native serial | Rust `TextStreamDecoder` in a std thread (`serial/mod.rs:79-103`) | 40 ms read timeout, 8 KiB buffer; a decode error kills the thread (`:93` uses `?`) and surfaces via `source_last_error` |
| RTT (probe-rs only) | Rust `TextStreamDecoder` (`controller.rs:240-279`) | up-channel read with 8 KiB buffer, 10 ms sleep when empty; holds the transport mutex per read |
| Memory poll (probe-rs or OpenOCD) | no text: `read_variables` builds one 1-sample batch per tick (`controller.rs:191-222`) | per-variable `core.read_8` (`:319-323`) or TCL `read_memory` (`:324-326`); 3 consecutive failures stop the worker (`:209-217`) |
| ELF symbols | not a carrier; supplies `DebugVariableDto{id,address,numericType,littleEndian}` (`DebugSourcePanel.tsx:35-40`) | |

Bytes to `Series`: every carrier yields `SampleBatch{sessionId,sequence,time:Float64Array,channels:Record<id,Float64Array>,droppedSamples,discontinuity?}` (`streaming/types.ts:17-25`). `useLiveStream.sink` posts each batch to the stream worker (`useLiveStream.ts:63-74`); the worker appends to `MultiChannelRingBuffer` and publishes `snapshotLatest(visibleSeconds)` at most every 33 ms with transferred buffers (`stream.worker.ts:16,39-52`). The main thread turns the snapshot into a `Dataset` of one `Series` per channel (`useLiveStream.ts:44-51`) and `useDataExplorer` re-runs the whole filter pipeline with `sourceRevision` = ring revision (`useDataExplorer.ts:80-88`), then decimates to 5 000 points using min/max indices of the first trace (`:151-163`).

Crossing Tauri IPC: pull, not push. There are no Tauri events or channels. `NativeQueueConnection.poll` calls `invoke("read_native_batches")` every 25 ms (`nativeQueue.ts:71-95`); the command drains the `VecDeque<NativeSampleBatch>` and returns it as JSON (`main.rs:199-217`), numbers arriving as `number[]` and re-wrapped into Float64Array (`nativeBridge.ts:36-52`). The same drain feeds the native recorder (`main.rs:206-215`), so recording only advances while the frontend polls. Errors are a second poll, `source_last_error`, which `take()`s a slot (`main.rs:219-240`). Queue depth is capped at 128 batches, oldest dropped silently (`serial/mod.rs:107-109`).

Timestamps: seconds relative to first sample of the connection. Browser decoders subtract the first `t` (scaled by `$schema.timeUnit` or column-name suffix) or fall back to `performance.now()` receive time with `timingWarning` (`ndjson.ts:154-173`, `delimited.ts:72-82`). Native decoder does the same with `Instant` elapsed (`text_decoder.rs:195-202`) but silently skips decreasing timestamps and always reports `dropped_samples: 0` (`:69-71,86`); browser decoders count and report them. Memory-poll batches use worker `Instant::now()` elapsed (`controller.rs:202`), i.e. host time, not target time.

## Catalog and variable discovery

`crates/herkules-dwarf` is a port of the `datavis-rs` parser (`docs/desktop-development.md:22-24`). `DwarfParser::parse_bytes` loads DWARF sections via `object` + `gimli` (`dwarf_parser.rs:274-300`), builds a `TypeTable` (structs, unions, arrays, enums, pointers, typedefs, C++ classes/templates: `type_table.rs:94-136`) and a symbol list with a `VariableStatus` explaining unreadable cases: optimized out, extern, register-only, implicit, multi-piece, artificial (`dwarf_parser.rs:29-56`). `parse_elf` (`elf.rs:31-78`) keeps globals only (`:41`), flattens struct members and arrays up to 4096 elements and depth 16 into child `SymbolDto`s with absolute addresses (`:80-125`), marks pointers unreadable with "not selectable in this release" (`:56,146-148`), and emits `numericType` from `PrimitiveDef::to_variable_type` (u8..f64, bool; `variable_type.rs:3-17`).

DTOs (`elf.rs:6-29`, mirrored in `src/backends/dwarf/types.ts`): `SymbolDto{name,address,addressHex,size,typeName,numericType?,readable,status,pointer,children}`, `ElfParseDto{path,littleEndian,symbols,totalVariables,readableVariables}`.

WASM adapter: `parse_elf_json` returns a JSON string (`herkules-dwarf-wasm/src/lib.rs:5-10`); `dwarf.worker.ts:11-12` `JSON.parse`s it; `browserParser.ts:9-24` spawns one worker per file and transfers the ArrayBuffer. Generated glue is checked in under `src/generated/dwarf-wasm/` (262 KB wasm) so `npm run dev` needs no Rust toolchain (`docs/web-backend.md:19-22`).

Data Explorer use: native `debug_choose_elf` opens rfd and parses (`debug/parser.rs:3-16`). `DebugSourcePanel` shows `SymbolTree` with checkboxes on readable leaves (`SymbolTree.tsx:16`), keyed by dotted path name; selected symbols become `DebugVariableDto`s where `id = symbol.name` and a heuristic fills `numericType` from `typeName` when the crate gave none (`DebugSourcePanel.tsx:82-89`). Browser build only inspects/searches (`BrowserDebugPanel.tsx:55` passes `selectable={false}`). Probe list is fetched only on the refresh button, never on mount (`DebugSourcePanel.tsx:42,72`).

## Write and tune path

None. The native backend is read-only by design (`docs/desktop-development.md:20`). Evidence: the command list (`main.rs:292-315`) has no write, halt, resume or set-variable command; `TclClient` implements only `read_memory` (`tcl_client.rs:61-72`); probe-rs attaches with `Permissions::new()` (`controller.rs:85`); `MemoryInterface` is imported for `read_8` only (`:9,322`). The UI has no editable value controls; the Inspector edits filter params, not target state.

Implications for tuning-tools: everything on the write side (typed `write_8/16/32/64`, atomicity of multi-byte writes on a running core, read-back verification, undo/snapshot of edited values, OpenOCD `write_memory`/`mww`, RTT down-channel or serial command paths, permission to halt) has to be designed from zero; nothing here constrains it. The one reusable piece is the `numeric_type`/`decode_number` pair (`controller.rs:340-374`), which needs an `encode_number` twin.

## Wire protocol

Streaming protocol v1 (`docs/streaming-protocol.md`): newline-delimited text, chunks need not align with lines. NDJSON row `{"t":0.001,"gyro_z":0.12}`, columnar batch `{"t":[...],"values":{"id":[...]}}`, optional `{"$schema":{"version":1,"timeUnit":"s|ms|us","channels":{"id":{"unit":..}}}}`. Keys starting with `$` and `t` are not channels (`ndjson.ts:115`). Missing values are NaN; decreasing timestamps rejected. Streaming CSV: first non-`#` line is the header; delimiter auto-detected among `, ; \t |` (`csv.ts:53-65`, `text_decoder.rs:145-148`); time column matched by `/^(t|ts|time|timestamp)(_|$)/i` in TS (`delimited.ts:63`) but by an exact list `t|ts|time|time_s|time_ms|timestamp` in Rust (`text_decoder.rs:158-163`), so `time_us` works in the browser and not natively. Gap tokens: `"" nan null none -`; `inf`, `true/false` accepted (`text_decoder.rs:205-214`). Producer guidance: 20-50 ms columnar batches at 1 kHz. Mock producer `scripts/mock-stream.mjs` (8 ch, 1 kHz, `ws://127.0.0.1:8765`, `--faults`).

OpenOCD TCL: commands are bytes terminated by `0x1a` (`tcl_client.rs:5,30-59`); a response starting with or containing `\nError:` is an error. The only command issued is `read_memory 0x{addr:x} 8 {count}` parsed as whitespace-separated hex bytes (`:61-72`). Launch args: `-s <scripts> -c "bindto 127.0.0.1" -c "tcl_port <ephemeral>" -c "gdb_port disabled" -c "telnet_port disabled" -f <interface.cfg> -f <target.cfg> [-f extra]` (`process.rs:36-60`); `.cfg` suffix validated (`:106-111`); waits up to 10 s for the TCL port, polling every 100 ms (`:72-88`). Attach mode defaults to `127.0.0.1:6666` (`DebugSourcePanel.tsx:23-24`). Bundled xPack OpenOCD 0.12.0-7 fetched by `scripts/fetch-openocd.mjs` with SHA-256 pins into `src-tauri/resources/openocd` (`docs/openocd-bundling.md`).

## Concurrency model

- All commands are plain sync `fn`s taking `State<'_, AppState>`; every field is a `std::sync::Mutex` (`app_state.rs:8-13`). Lock poisoning is mapped to string errors everywhere (`main.rs:53` pattern).
- One std thread per active acquisition: `SerialController::start` (`serial/mod.rs:45-63`), `start_memory_polling` (`controller.rs:185-223`), `start_rtt` (`:240-279`). Stop is an `AtomicBool` plus `join()` (`serial/mod.rs:65-70`, `controller.rs:283-288`); `Drop` stops workers (`serial/mod.rs:73-77`, `controller.rs:298-302`).
- Probe/OpenOCD ownership: `DebugController.transport: Arc<Mutex<Option<DebugTransport>>>` (`controller.rs:29`) is shared between the worker thread (locks per tick or per RTT read) and command handlers (`list_rtt_channels` locks it too). `connect_*` always `disconnect()`s first, so one transport at a time (`:73,96,113`). `OpenOcdProcess::drop` kills the child (`process.rs:99-104`); stdout/stderr are captured by two threads into a 200-line ring (`:126-137`).
- Batch handoff: producers call `push_bounded` on the shared `VecDeque` (`serial/mod.rs:105-112`); the consumer is the frontend's 25 ms `read_native_batches` poll.
- Recording writer: `NativeRecorder` is owned by `AppState.recorder` and written on the IPC thread during `read_native_batches` (`main.rs:206-215`), not by the acquisition thread. `recording_start` blocks inside an rfd save dialog (`main.rs:247-253`).
- Blocking inside commands: OpenOCD launch spin-wait up to 10 s (`process.rs:72-88`), TCL connect 5 s / read 10 s timeouts (`tcl_client.rs:18-25`), `serial_connect` joins the old worker (`serial/mod.rs:40`).
- Frontend: `useLiveStream` keeps `LiveConnection`, `AbortController`, worker client and recorder in refs (`useLiveStream.ts:31-36`); `replaceConnection` aborts and disconnects the previous one before starting the next (`:76-87`).

## UI model

Layout: `react-resizable-panels` three-column page: Source + Pipeline rail (20 %), `ChartStack` (58 %), Inspector (22 %) (`DataExplorerPage.tsx:17-85`). Routes are lazy per tool (`App.tsx:8-17`); `config/tools.ts` drives home page and nav.

Source chooser: four tabs `file | websocket | serial | debug` (`SourceChooser.tsx:12,44-46`); switching tabs disconnects a live source (`:39`). `ConnectionPanel` holds URL/baud/format with defaults `ws://127.0.0.1:8080`, 115200, ndjson (`ConnectionPanel.tsx:18-20`). `DebugSourcePanel` picks transport (`direct | attach | launch`) and acquisition (`memory | rtt`), defaults `STM32F407VGTx`, `interface/stlink.cfg`, `target/stm32f4x.cfg`, 100 Hz (`DebugSourcePanel.tsx:22-27`), then `connect -> start_* -> attachDebugQueue` (`:49-63`). `StreamStatus` shows phase, Hz, RX, drops and the last two issues (`StreamStatus.tsx`).

Pipeline: state is `ExplorerState{dataset,datasetRevision,channel,pipeline,taps,selectedNode,chartMode}` (`state.ts:5-16`); node ids are a monotonic counter (`:35-39`). `evaluate` walks the topological order with a per-node cache keyed by `kind|params|upstreamKey` (`pipeline.ts:203-221`); source keys include `sourceRevision` so live data invalidates correctly (`:178-182`). Although stored as a DAG, evaluation reads `inputs[0]` only (`:185`), and `reorderChain` rewires to a strict chain (`:263-276`). Adding a filter taps the new node and untaps its predecessor, keeping the source tapped (`useDataExplorer.ts:165-183`). Inspector renders controls from `ParamSpec` (number with optional log slider, select, `visibleWhen`) (`filters/types.ts:24-39`, `Inspector.tsx:10-13`). Colours come from `bandFor(index)` (resistor-code palette, `core/palette.ts`) shared by rail and chart.

uPlot wrapper (`Plot.tsx:33-126`): React owns the host div, uPlot the canvas; rebuild only when the structure string (labels+colours+syncKey) changes, else `setData` (`:50-62`); `scales.x.time=false` (`:74`); grid disabled in uPlot and drawn as a CSS graticule positioned from `u.bbox` (`:133-142`); `ResizeObserver` resizes; cursor `sync.key` shares crosshairs in stacked mode (`ChartStack.tsx:23,60`). Points appear only when fewer than 60 samples are visible (`:102`). Legend is hand-rendered from traces (`ChartStack.tsx:108-120`).

Tailwind: theme tokens as HSL CSS variables (`index.css:8-25`), single accent colour, IBM Plex Mono / Archivo Narrow from Google Fonts (`index.css:1`); `tailwind.config.ts` defines the graticule gradient (`:64-65`). Radix primitives + `class-variance-authority` button.

## Persistence

- Recipe only: `PersistedState{v:1,channel,nodes[{id,kind,params,inputs}],taps,mode}` (`persist.ts:12-18`), validated on read (`:43-68`). Written to `localStorage["herkules.dataexplorer.v1"]` and to the URL hash as base64url JSON via `history.replaceState` on every change (`useDataExplorer.ts:49-59`); restored hash-first then local (`:36-46`). Data (CSV) is never persisted.
- Recording, browser: `BrowserCsvRecorder` writes `time_s,<ids>` CSV through `showSaveFilePicker` or buffers chunks and triggers a download at stop (`browserRecorder.ts:42-65,111-120`); new channels mid-recording abort it (`:69-74`).
- Recording, native: `NativeRecorder` writes to `<name>.csv.partial`, NaN as empty cell, renames on `finish` (`recording/mod.rs:14-63`). `recording_recoverable_files` lists `*.partial` in Downloads (`main.rs:274-287`) but no frontend code calls it. Native byte/sample stats are estimated in TS (`useLiveStream.ts:66-70`), not reported by Rust.
- No settings store, no connection-profile persistence, no Tauri plugin-store; window size only from `tauri.conf.json`.

## Numbers

| Item | Value | Anchor |
|---|---|---|
| Native poll of batch queue | every 25 ms | `nativeQueue.ts:94` |
| Native batch queue cap | 128 batches, drop oldest | `serial/mod.rs:107` |
| Serial read timeout / buffer | 40 ms / 8192 B | `serial/mod.rs:85,89` |
| Serial baud range | 300..4 000 000 | `serial/mod.rs:37`, `browserSerial.ts:52` |
| Memory poll rate | 1..1000 Hz, sleep to interval | `controller.rs:177,186,219-221` |
| Poll failure tolerance | 3 consecutive | `controller.rs:211` |
| RTT read buffer / idle sleep | 8192 B / 10 ms | `controller.rs:243,269` |
| OpenOCD start deadline / poll | 10 s / 100 ms | `process.rs:72,82` |
| OpenOCD log ring | 200 lines | `process.rs:64,130` |
| TCL connect / read / write timeouts | 5 s / 10 s / 5 s | `tcl_client.rs:18-25` |
| TCL read chunk | 4096 B | `tcl_client.rs:40` |
| Ring buffer capacity / visible window | 60 000 samples / 10 s | `streamWorkerClient.ts:14`, `stream.worker.ts:12-13` |
| Worker publish rate | 30 Hz (33.3 ms) | `stream.worker.ts:16` |
| Plot decimation threshold | 5 000 points | `useDataExplorer.ts:152-155` |
| WebSocket reconnect delays | 0.5, 1, 2, 5, 10 s; reset after 10 s uptime | `websocket.ts:138-141` |
| Issue list kept in UI | last 5 (`slice(-4)` + new) | `useLiveStream.ts:96` |
| Browser fallback recorder limit | 100 MB or 5 min | `browserRecorder.ts:77` |
| Mock stream | 1 kHz, 8 ch, 20 ms batches default; backpressure at 1 MB buffered | `scripts/mock-stream.mjs:50-51` |
| DWARF flatten limits | arrays <= 4096 elements, depth 16 | `elf.rs:88,110` |
| Recommended producer batch | 20-50 ms at 1 kHz | `docs/streaming-protocol.md` |
| CSV jitter warning threshold | 25 % of mean dt | `csv.ts:26` |

## Reuse verdict for tuning-tools

Carry over verbatim
- `crates/herkules-dwarf` whole (parser, type table, `VariableStatus`, `parse_elf`/`SymbolDto`) plus its ELF fixtures under `tests/fixtures`. It is the mature part (4.2k lines, tests) and already emits address + numeric type per leaf.
- `SampleBatch` shape and the `LiveSource`/`LiveConnection`/`SourceStatus` contracts (`core/streaming/types.ts`), `MultiChannelRingBuffer`, `decimation.ts`.
- `Plot.tsx` structure-key/`setData` pattern and CSS graticule; `ChartStack` overlay/stacked with cursor sync.
- `persist.ts` shape validation + base64url hash approach.
- `text_decoder.rs` `parse_cell` and `controller.rs` `numeric_size`/`decode_number` (add encode).
- `openocd/process.rs` launch arguments and port handling; `tcl_client.rs` framing.

Redesign
- IPC transport: replace the 25 ms `read_native_batches` JSON pull (`nativeQueue.ts:71-95`, `main.rs:199-217`) with Tauri `Channel`/events or a binary frame (`Vec<f64>` over `tauri::ipc::Response`), and move recording off the IPC thread.
- Memory polling: one `core(0)` acquisition and one `read_8` per variable per tick (`controller.rs:315-327`) and one TCL round trip per variable (`tcl_client.rs:62`) will not scale past a handful of variables at 1 kHz; coalesce adjacent addresses into block reads and timestamp from the target when possible.
- Error reporting: replace the `take()` slot polled by `source_last_error` (`main.rs:219-240`) with a status event stream; native `dropped_samples` is always 0 (`text_decoder.rs:86`).
- Native vs browser decoder drift (time-column regex, `time_us`, fatal vs counted malformed lines: `text_decoder.rs:98` vs `ndjson.ts:59-64`). Keep one decoder (Rust) and reuse it via WASM the way the DWARF crate is.
- `DebugVariableDto` is abused as a channel list for `recording_start` with `address: 0, numericType: "f64"` (`useLiveStream.ts:138`); separate recording channels from poll targets.
- Sync commands that block (rfd dialog `main.rs:247`, 10 s OpenOCD wait): use async commands and the dialog plugin.
- Pipeline is DAG-on-disk, chain-in-practice (`pipeline.ts:185`); either commit to the chain or implement multi-input.

Drop
- `src/tools/md2wangeditor/**`, `sampleData.ts` (808 lines), Cloudflare `wrangler.toml`, the whole hosted/WebSerial/browser-recorder path if tuning-tools is desktop-only.
- `nativeBridge.ts` `__TAURI_INTERNALS__` probing; use `@tauri-apps/api` directly.

Unfinished or stubbed
- No write path at all (see "Write and tune path").
- `debug/openocd/mod.rs` is 2 `pub mod` lines; `debug/parser.rs` is 16 lines of file-dialog glue; `herkules-dwarf-wasm/src/lib.rs` is 10 lines.
- Commands registered but never invoked by the frontend: `recording_recoverable_files`, `debug_stop_polling`, `debug_openocd_logs`, `serial_stop` (no recovery UI, no OpenOCD log viewer, no stop-without-disconnect).
- `SampleBatch.discontinuity` and `DecoderOutput.timingWarning` are produced by decoders but consumed nowhere outside tests.
- `Source` interface in `core/series.ts:25-28` ("CSV today; WebSocket/WebSerial later") is dead.
- `OpenOcdLaunchDto.extra_config_files` is always `[]` from the UI (`DebugSourcePanel.tsx:47`); `RttChannelDto.bufferSize` unused.
- Pointers deliberately unreadable "in this release" (`elf.rs:148`); `VariableStatus::address()` returns placeholder 0 for multi-piece (`dwarf_parser.rs:50-54`).
- Single-channel filter pipeline: one `channel` feeds the chain (`state.ts:9-11`); multi-channel tuning views do not exist.
- Design spec with deferred roadmap: `docs/superpowers/specs/2026-08-03-herkules-tools-design.md`.
