# RM Studio (PC + MCU)

Source: `https://gitee.com/ZZY-YES/rm-studio-pc` (Python 3 / PyQt5 + QWebEngine + QWebChannel, JS front end; **GPL-3.0-or-later**) and `https://gitee.com/ZZY-YES/rm-studio-mcu` (C++17, no HAL/RTOS/heap; **LGPL-3.0-or-later**). Both v2.5.6, author zzy (HKBU Navigator Beta), article `01M0YXWNA7NET0XTKNX5JMETXE` on bbs.herkules.dev. Sizes: PC ~3.1k lines Python + ~1.5k lines HTML/JS/CSS; MCU ~3.5k lines C++ (include+src). Local clones under `scratchpad/rm-studio-pc` and `scratchpad/rm-studio-mcu`; `file:line` anchors below point into them.

**Licence rule: layout and ideas only.** This note records wire layouts, state machines, numbers and design decisions so tuning-tools can implement them independently in Rust. No code, comments, or UI text is to be copied from either repository. GPL/LGPL obligations attach to their code, not to the protocol facts documented here.

## Purpose

A whole-vehicle tune/observe workbench: the MCU publishes a parameter catalog and a watch catalog; the PC generates the UI from them, writes parameters into a session, saves them to flash, subscribes to variables at chosen rates, and shows live values, scope curves, RTOS task snapshots and trace timelines. Design constraints stated in the article (lines 540-542, 605-621): registries live on the MCU, the PC never bypasses MCU safety, one physical link carries control and data traffic, and rendering must not stall on data rate.

## Module map

PC (`rm-studio-pc/`):

| Path | Responsibility |
|---|---|
| `tune_studio/protocol.py` | Frame codec, CRC16, command/status enums, payload parsers for TU/v2 and RM/v1 |
| `tune_studio/client.py` | `StudioClient`: single RX thread, request/response matching, async sample/trace queues, both sessions |
| `tune_studio/transports.py` | `Transport` ABC; pyserial wrapper, 921600 8N1, fixed 20 ms read timeout (`:26-34`, `:48-56`) |
| `tune_studio/mock_transport.py` | In-process MCU simulator (params, watches, trace, tasks, sessions) for `--demo` |
| `tune_studio/qt_backend.py` | `TuneBackend` QObject: timers, data/control worker, bounded series ring, display packets, Qt signals to JS |
| `tune_studio/qt_app.py`, `launcher.py` | QWebEngineView + QWebChannel bootstrap |
| `tune_studio/json_io.py`, `c_export.py` | Config JSON export/import; "solidify" Markdown report |
| `tune_studio/web/index.html`, `js/app.js` | 7-view single page UI, scope renderer, trigger recorder, protocol probe |
| `protocol_probe.py` | CLI: send TU/RM HELLO or raw hex, dump raw bytes |

MCU (`rm-studio-mcu/`):

| Path | Responsibility |
|---|---|
| `include/tune/param.hpp` | `ParamDescriptor`, `WritePolicy`, `VehicleState`, FNV-1a stable id |
| `include/tune/bind.hpp` | `TUNE_BIND_*` macros; place descriptor in section `rm_tune_params` |
| `include/tune/value.hpp` | 8-byte value codec, finite check |
| `include/tune/registry.hpp`, `src/tune/registry.cpp` | Linker-section registry, read/write/validate, fingerprint |
| `include/tune/config_store.hpp`, `src/tune/config_store.cpp` | Default/Saved/Current, A/B slot snapshot, generation, CRC |
| `include/tune/session.hpp` | Lease: open/touch/timeout |
| `include/tune/frame.hpp`, `crc.hpp`, `src/tune/frame.cpp`, `crc.cpp` | TU framing and CRC16 |
| `include/tune/service.hpp`, `src/tune/service.cpp` | TU/v2 command handler, policy check |
| `include/rm_studio/watch.hpp`, `bind.hpp`, `src/watch.cpp` | Watch descriptors (section `rm_studio_watch`), subscription table, bandwidth estimate |
| `include/rm_studio/trace.hpp`, `src/trace.cpp` | Trace descriptors (section `rm_studio_trace`), drop-on-contention ring |
| `include/rm_studio/monitor_service.hpp`, `src/monitor_service.cpp` | RM/v1 handler, sample/trace emitters, budget, stats |
| `include/rm_studio/runtime.hpp` | Optional RTOS task snapshot provider |
| `include/rm_studio/io.hpp`, `budget.hpp`, `config.hpp` | Transport/Clock/SafetyProvider callbacks, budget, compile-time limits |
| `include/rm_studio/studio_runtime.hpp`, `src/studio_runtime.cpp` | `ProcessOneSlice`: drain RX, poll both services, write TX |

```
Web UI (app.js) --QWebChannel signals/slots--> TuneBackend (qt_backend.py)
    |  Qt thread: timers only                      |  data/control worker: queued requests + pump_async
    v                                              v
StudioClient (client.py) --encode--> Transport (serial 921600) ==bytes==> MCU
    ^  one RX thread demuxes TU/RM frames                                 |
    |                                                       StudioRuntime::ProcessOneSlice (low-prio task)
    |                                                          |-> tune::Service  -> ConfigStore -> Registry (rm_tune_params)
    +------ SAMPLE / TRACE_EVENT async frames <---------------- MonitorService -> WatchRegistry (rm_studio_watch)
```

## Acquisition path

- **Selective Watch.** Nothing streams until the PC `SUBSCRIBE`s an id with a requested Hz; the MCU clamps to `max_hz`, assigns a slot (≤12), trial-adds it and rejects with `BUDGET` if the estimated byte rate exceeds the budget (`monitor_service.cpp:187-224`, `watch.cpp:65-105`). Estimate = Σhz×10 B + min(Σhz,1000)×15 B (`watch.cpp:167-190`).
- **SAMPLE frames.** Each poll packs every due slot into one frame `count | timestamp_us | (slot,type,raw[8])*` and advances `next_due` by whole periods (`monitor_service.cpp:388-444`). Values are read as a volatile byte copy so ISR-updated variables are safe (`watch.cpp:55-62`). One TX frame in flight per service.
- **Control plane vs data plane.** Request/response frames are matched by (magic, cmd|0x80, seq) into a pending deque; `SAMPLE`/`TRACE_EVENT` go to an async deque (maxlen 4096) that requests never consume (`client.py:195-213`). `pump_async` drains it without touching the serial port (`client.py:635-657`).
- **PC retention.** Data worker keeps per-path `deque(maxlen=24000)` plus a latest dict (`qt_backend.py:112-114, 221-243`) and builds a display packet at ≤30 Hz (15 Hz wireless, 20/24 Hz with ≥6/≥4 signals), max 8 paths, envelope-downsampled to the scope pixel budget over a 0.5-60 s window (`qt_backend.py:286-352`). A 16 ms Qt timer flushes the packet to JS (`:137, :947-966`).
- **Rendering cadence.** Latest-value cards re-render at 100 ms (`app.js:154, 240`). Scope draws on `requestAnimationFrame` gated at 33 ms, raised to 40 ms if the average draw time >14 ms and 50 ms if >22 ms (`app.js:153, 210, 245`).
- **Stats.** `GET_INFO` returns `max_bps` and `estimated_bps`; `GET_STATS` returns tx/rx bytes, sample frames, dropped frames, trace events, dropped trace, subscription rejects (`monitor_service.cpp:104-121, 242-254`). UI shows active count, estimated/max bandwidth, dropped.
- **PC Trigger Recorder.** Runs entirely on already-received samples: arm on a subscribed path with `>`, `<`, `abs>` and a threshold; on hit it slices `monitorSeries` to [-pre, +post] seconds (defaults 0.5/1.0 s) and freezes it (`app.js:158, 251-253`; `index.html:277-286`). Zero MCU cost.
- **Trace.** `RM_TRACE_EVENT/SCOPE` push `{ts_us,id,value,kind}` into a 48-entry ring only when enabled; contention or full ring drops and counts, never blocks (`trace.cpp:24-58`). Emitter batches up to 8 records per `TRACE_EVENT` frame (`monitor_service.cpp:446-484`). RTOS task snapshot is pulled only on `RUNTIME_TASKS` requests, paged 7 at a time (`runtime.hpp:29-39`, `monitor_service.cpp:270-329`); PC polls at 500 ms (1000 ms wireless) only while the runtime view is open (`qt_backend.py:143, 672, 1115-1125`).

## Catalog and variable discovery

- **Registry.** Each `TUNE_BIND_*` expands to a `const ParamDescriptor` in an anonymous namespace with `__attribute__((used, section("rm_tune_params"), aligned(4)))` (`bind.hpp:8-26`); the registry is the `__start_/__stop_` range, declared weak so an empty section links (`registry.cpp:11-20`). Same scheme for `rm_studio_watch` (`bind.hpp` under `rm_studio/`, `watch.cpp:8-17`) and `rm_studio_trace` (`trace.hpp:137-148`).
- **Descriptor fields** (`param.hpp:35-52`): `id u32` (FNV-1a 32 of `component + '.' + name`, `:54-67`), `component`, `name`, `unit`, `type` (F32=1,I32=2,U32=3,Bool=4), `flags` (Readable=1, Persistent=2, LegacyMutableBinding=4), `policy`, `value_ptr`, `factory` value, `min`, `max`, `step`, `max_step` (0 = no per-write delta limit), `apply` fn + ctx. The factory value is captured from the variable's initialiser at bind time (`bind.hpp:24-25`). **Fingerprint** = CRC16 over `id u32 | type u8 | factory[8]` (`registry.cpp:132-138`); registry **signature** = 0xFFFF XOR all fingerprints, order-independent (`:140-149`).
- **Watch descriptor** (`watch.hpp:29-38`): `id`, `group`, `name`, `unit`, `address` (const volatile), `type` (F32..U8, 8 kinds), `recommended_hz`, `max_hz`.
- **Paging.** `CATALOG_PAGE(start u16, count u8)` → `total u16, returned u8, records…`; MCU clamps count to ≤8 (TU) / ≤10 (RM) and stops early if a record would overflow the frame (`service.cpp:155-228`, `monitor_service.cpp:124-185`). PC asks for 2 per page, retries each watch page twice with 10 ms×n backoff, and rejects a catalog whose `total` changes mid-walk or exceeds 512 (`client.py:288-300, 371-435`).
- **UI generation.** The PC groups by `component`/`group`, renders `path = component.name`, uses `min/max/step` for slider and numeric input, `unit` for labels, `policy` for write-allowed hints, and the watch `recommended_hz` as the default subscribe rate (rate drafts persisted in localStorage, `app.js:14-15`).

## Write and tune path

- **Commands.** `READ_PARAM` returns current and saved side by side; `WRITE_PARAM` alters only the session (Current); `SAVE` commits, `DISCARD` restores Saved, `RESET_FACTORY` restores descriptor defaults and saves (`service.cpp:230-344`).
- **Policy on MCU** (`service.cpp:69-88`): `ReadOnly` or vehicle `Fault` → deny; `Live` → allow; `SafeOnly` → `Kill|Safe`; `KillOnly` → `Kill`. `SAVE` and `RESET_FACTORY` additionally require `Kill|Safe` (`:324, :338`). Check order for a write: found → not ReadOnly → policy vs state → type matches → decodes finite → within `[min,max]` → `|new-cur| ≤ max_step` → apply (`:271-319`). Error codes map 1:1: `READ_ONLY`, `STATE_DENIED`, `BAD_PAYLOAD`, `RANGE`, `STEP_TOO_LARGE`; `VALIDATION` is returned when the optional store validator rejects a `SAVE` (`:329`). Both docs and article stress the PC's greyed buttons are advisory only.
- **Apply callback.** Invoked in the tune task right after the pointer write (`registry.cpp:101-113`), also on rollback (`config_store.cpp:24-30`). Guidance (`bind.hpp:64-66`, `PARAMETERS_CN.md`): set a pending flag; the owning control task re-initialises its PID/limits in its own context.
- **Three layers.** Default = descriptor factory; Saved = `committed_[]` captured after load/save; Current = the bound variable. `expected_[]` tracks what the tune layer last wrote; `GuardBoundValues` restores and counts `guard_violations` when firmware code changes a bound variable behind its back (`config_store.cpp:41-53`). `dirty` = Current ≠ Saved.
- **A/B storage.** See Persistence. Save builds snapshot with generation+1, erases and writes the *inactive* slot, reads it back and verifies, then flips `active_slot` (`config_store.cpp:251-282`).
- **Session lease.** `HELLO` opens a lease (id increments, never 0) with `timeout_ms` (1200 default); every non-HELLO frame touches it; non-HELLO without lease → `SESSION_REQUIRED`; timeout → `DiscardSession` (rollback to Saved, apply callbacks fire); `GOODBYE` does the same explicitly (`session.hpp`, `service.cpp:90-120, 365-369`). The monitor lease is independent: timeout clears subscriptions and disables trace (`monitor_service.cpp:59-63, 486-491`). PC heartbeats both leases every 400 ms (500 wireless) with 150 ms request timeouts, treats them as separate failures, and re-HELLOs + resubscribes on `SESSION_REQUIRED` (`qt_backend.py:536-537, 760-800, 815-870`).

## Wire protocol

Both protocols share one frame codec; only magic and version differ. All multi-byte fields little-endian.

| Offset | Size | Field | Notes |
|---|---|---|---|
| 0 | 2 | magic | `"TU"` (tune, v2) or `"RM"` (monitor, v1) |
| 2 | 1 | version | 2 for TU, 1 for RM (`protocol.py:10-11, 242-243`; `frame.cpp:13-15`; `monitor_frame.cpp:13-15`) |
| 3 | 1 | command | request id; response = request \| 0x80; async MCU→PC frames use their own ids |
| 4 | 2 | sequence | u16; PC counters run 1..0xFFFF skipping 0, one per protocol (`client.py:157-164`); MCU echoes it; async frames use a separate MCU counter |
| 6 | 2 | payload_length | u16 |
| 8 | n | payload | response payload always starts with `status u8` (`service.cpp:54-67`) |
| 8+n | 2 | crc16 | over bytes 0..8+n |

CRC16: reflected polynomial 0x8408 (CCITT reversed), init 0xFFFF, no final XOR, LSB-first shift (`crc.cpp:13-25`, `protocol.py:112-119`) — i.e. CRC-16/MCRF4XX. Resync on bad magic/version/length/CRC = drop one byte and retry (`protocol.py:127-150`, `frame.cpp:69-116`). Max frame 512 B (PC and TU parser); the RM parser buffer is `RM_STUDIO_RX_BUFFER` = 256 B (`config.hpp:26-31`), so monitor frames must fit 256. Frames of the other protocol are simply resynced past by each MCU parser; the PC demuxes both magics in one buffer (`client.py:166-193`).

Value encoding (`value.hpp:93-132`, `protocol.py:153-176`): fixed 8 bytes; F32/I32/U32 in bytes 0-3 zero-padded, Bool in byte 0; watch I16/U16/I8/U8 likewise low-first. Strings are length-prefixed, no terminator, ≤255 (TU) / ≤63 (RM).

TU/v2 commands (`protocol.py:13-24`, `service.hpp:16-28`; layouts `service.cpp:96-375`, `protocol.py:179-239`, `client.py:271-329`):

| Id | Name | Request payload | Response payload (after status) |
|---|---|---|---|
| 0x01 | HELLO | – | `session_id u32, timeout_ms u16` |
| 0x02 | HEARTBEAT | – | `session_id u32` |
| 0x03 | GET_INFO | – | `schema u16, store_fmt u16, factory_sig u16, hb_timeout u16, generation u32, param_count u16, dirty u8, active_slot u8 (0xFF none), session u8, vehicle_state u8, guard_violations u32, name_len u8, name` |
| 0x04 | CATALOG_PAGE | `start u16, count u8` (≤8) | `total u16, returned u8`, then per record: `id u32, type u8, flags u8, policy u8, comp_len u8, name_len u8, unit_len u8, rsv u8, min f32, max f32, step f32, max_step f32, fingerprint u16, factory[8], comp, name, unit` (37 B + strings) |
| 0x05 | READ_PARAM | `id u32` | `id u32, type u8, pad[3], current[8], saved[8]` (24 B) |
| 0x06 | WRITE_PARAM | `id u32, type u8, pad[3], value[8]` (16 B) | – |
| 0x07 | SAVE | – | – (Kill/Safe only; PC waits 2 s) |
| 0x08 | DISCARD | – | – |
| 0x09 | RESET_FACTORY | – | – (Kill/Safe only) |
| 0x0A | GET_STATUS | – | `generation u32, dirty u8, active_slot u8, session u8, vehicle_state u8, hb_age_ms u16 (sat), hb_timeout u16, guard_violations u32` (16 B) |
| 0x0B | GOODBYE | – | – (discard + close) |

TU status (`protocol.py:26-38`): OK 0, BAD_COMMAND 1, BAD_PAYLOAD 2, NOT_FOUND 3, READ_ONLY 4, RANGE 5, STORAGE 6, BUSY 7, SESSION_REQUIRED 8, STATE_DENIED 9, STEP_TOO_LARGE 10, VALIDATION 11. Enums: ParamType F32 1/I32 2/U32 3/BOOL 4; WritePolicy LIVE 0/SAFE_ONLY 1/KILL_ONLY 2/READ_ONLY 3; VehicleState KILL 0/SAFE 1/ARMED 2/RUNNING 3/FAULT 4 (`protocol.py:40-57`).

RM/v1 commands (`protocol.py:245-250`, `monitor_service.hpp:19-35`; layouts `monitor_service.cpp:65-484`, `client.py:341-624`):

| Id | Name | Request payload | Response / frame payload |
|---|---|---|---|
| 0x01 | HELLO | – | `session_id u32, timeout_ms u16` |
| 0x02 | HEARTBEAT | – | `session_id u32` |
| 0x03 | GET_INFO | – | `proto u16 (=1), watch_count u16, active u8, trace_enabled u8, max_bps u32, estimated_bps u32, name_len u8, name` |
| 0x04 | CATALOG_PAGE | `start u16, count u8` (≤10) | `total u16, returned u8`, per record: `id u32, type u8, rec_hz u16, max_hz u16, g_len u8, n_len u8, u_len u8, group, name, unit` (14 B + strings) |
| 0x05 | SUBSCRIBE | `id u32, hz u16` | `slot u8, actual_hz u16, estimated_bps u32`; errors NOT_FOUND / BUSY (no slot) / BUDGET |
| 0x06 | UNSUBSCRIBE | `id u32` | – |
| 0x07 | CLEAR_SUBSCRIPTIONS | – | – |
| 0x08 | GET_STATS | – | `tx_bytes, rx_bytes, sample_frames, dropped_frames, trace_events, dropped_trace, subscription_rejects, estimated_bps` (8×u32) |
| 0x10 | TRACE_ENABLE | `enabled u8` | – (disable also clears ring) |
| 0x11 | TRACE_CLEAR | – | – |
| 0x12 | GOODBYE | – | – |
| 0x20 | RUNTIME_TASKS | `start u16, count u8` (optional, ≤7) | `total u16, returned u8, total_runtime u32`, per task: `id u32, runtime u32, stack_free u16, prio u8, state u8, name_len u8, name`; NOT_FOUND if no provider |
| 0x21 | TRACE_CATALOG_PAGE | `start u16, count u8` (optional, ≤4) | `total u16, returned u8`, per record: `id u32, name_len u8, name` |
| 0x40 | SAMPLE (async MCU→PC) | – | `count u8, timestamp_us u32`, then `slot u8, type u8, raw[8]` × count |
| 0x41 | TRACE_EVENT (async) | – | `count u8`, then `ts_us u32, id u32, value u32, kind u8` × count (≤8); PC also accepts a bare 13 B single record (v2.3 compat) |

RM status (`protocol.py:253`): OK 0, BAD_COMMAND 1, BAD_PAYLOAD 2, NOT_FOUND 3, BUSY 4, SESSION_REQUIRED 5, BUDGET 6. WatchType: F32 1, I32 2, U32 3, BOOL 4, I16 5, U16 6, I8 7, U8 8 (`watch.hpp:14-23`). TraceKind: Event 1, ScopeBegin 2, ScopeEnd 3, TaskIn 4, TaskOut 5, IrqIn 6, IrqOut 7 (`trace.hpp:16-24`); trace id = FNV of `"trace." + name` (`trace.hpp:39-41`). Task state: Running 0..Invalid 5 (`runtime.hpp:11-18`).

HELLO/HEARTBEAT semantics: HELLO always succeeds and starts a new lease (old subscriptions/session changes are not preserved on the TU side; RM keeps the table until timeout). HEARTBEAT is the only mandatory keep-alive but any frame touches the lease. Both services answer `SESSION_REQUIRED` to everything else without a lease.

## Concurrency model

- **PC.** Exactly two background threads (`qt_backend.py:100-104`): the `StudioClient` RX thread, which is the only caller of `transport.read()` and demuxes frames into pending/async deques under one condition variable (`client.py:57-111`); and a data/control worker with high/normal deques (maxlen 128/256) whose entries are keyed so repeated heartbeat/status/runtime requests coalesce (`:186-215`). Requests serialise on an RLock with a 2 ms minimum gap and wait on the CV for the matching (magic, cmd|0x80, seq) (`client.py:215-259`). The worker alternates one queued request with a `pump_async` (≤1024 frames, 5-20 ms wait) and a display-packet build (`qt_backend.py:354-390`). The Qt thread runs timers only. A "manual capture" mode lets the probe send raw bytes while the same RX thread collects raw bytes instead of frames (`client.py:113-154`). The serial read timeout is set once at open; it is never changed from the RX thread because pyserial reprogramming COMMTIMEOUTS on Windows disturbs the link (`transports.py:48-56`).
- **MCU.** `ProcessOneSlice` (`studio_runtime.cpp:14-63`): read up to 16×256 B chunks, feed each chunk to both parsers, `Poll()` both services (lease timeout, sample/trace emission), then write the tune TX frame first and the monitor TX frame second. Each service holds one TX buffer and ignores new requests while it is full (`service.cpp:97`, `monitor_service.cpp:66`). Intended host: one low-priority RTOS task woken by RX and by `RecommendedWaitMs()` (∞ without session, ≤2 ms with trace enabled, else time to next due sample; `monitor_service.hpp:80-104`). `HasBackgroundWork()` = session active and subscriptions present (`studio_runtime.hpp:29-31`).
- **Motor safety.** The tune task only writes the bound variable and calls `apply`; `apply` is documented to set a pending flag consumed by the owning control task (`bind.hpp:64-66`, article line 156). Trace probes use a try-lock and drop instead of waiting (`trace.cpp:29-35`). Watch reads are bounded volatile byte copies (`watch.cpp:55-62`).

## UI model

Views (`index.html:50-56`): **overview** (device, lease, vehicle state, param count, dirty, generation), **parameters** (module rail, search, favorites/recent, inspector with Default/Saved/Current and policy), **realtime** (watch picker with per-variable rate, latest cards, scope with window 0.5-60 s, PC Trigger Recorder, acquisition stats), **runtime** (RTOS task snapshot with poll interval 100-5000 ms, execution timeline from trace scopes/events, duration trigger `app.js:150`), **safety** (supervisor state, lease/heartbeat, policy legend, guard violations), **storage** (three-layer explanation, A/B slot, save/discard/reset, export/import), **logs** (filtered log, raw TX/RX capture, protocol probe: TU HELLO / RM HELLO / raw hex `index.html:395-401`).

- **Catalog signature.** The watch list DOM is rebuilt only when a signature of (filtered items, query, wireless flag) changes, because heartbeat-driven state updates arrive continuously and replacing a `<select>` while open collapses it under QWebEngine (`app.js:179-184`).
- **Edit lock.** Entering edit mode snapshots a baseline; background `readAll` updates never overwrite the draft; leaving requires apply or cancel (`app.js:11, 111-121, 128-130`).
- **Export/import.** JSON format `rm-tune-config-v2` with device name/schema/signature/generation and per-param id/path/type/policy/unit/factory/saved/value/fingerprint (`json_io.py:10-27`). Import writes each matching path via `WRITE_PARAM` into the session, subject to MCU checks (`qt_backend.py:1433-1443`). The "C header" export is really a Markdown solidify checklist telling the developer to edit the initialiser in source; fingerprints then migrate automatically (`c_export.py:8-30`).

## Persistence

`StorageBackend` = `{ctx, slot_size, read(slot,off,dst,len), erase(slot), write(slot,off,src,len)}`, two independent slots (`storage.hpp:11-17`). Snapshot (`config_store.hpp:26-48`, packed):

| Struct | Layout |
|---|---|
| `StoreHeader` (20 B) | `magic u32 = 0x454E5554 "TUNE", format_version u16 = 2, schema_version u16 = 2, generation u32, param_count u16, payload_size u16, crc16 u16 (computed with this field zeroed, over header+records), reserved u16` |
| `StoreRecord` (16 B) | `id u32, type u8, size u8 (=8), factory_fingerprint u16, value[8]` — only `Persistent` params |

Boot (`config_store.cpp:183-236`): set all factory; load both slots (magic, format, size, CRC checked); none valid → save factory (`InitializedFactory`); pick the newer generation by signed 32-bit difference; schema mismatch → factory + save (`SchemaReset`); per record, skip unknown id / type mismatch / fingerprint mismatch (firmware default changed → keep new default) / out-of-range value, then resave if any default changed (`FactoryDefaultsUpdated`). Save: generation+1 into the inactive slot, read-back verify, flip. Limits: `TUNE_MAX_PARAMS` 128, `TUNE_MAX_SNAPSHOT` 2304 B (`tune_config.hpp:8-16`); `slot_size` ≥ snapshot. Guidance: save only on explicit user action, never from ISR (`FLASH_STORAGE_CN.md`, cheat sheet).

## Numbers

| Item | Value | Where |
|---|---|---|
| Baud | 921600, read timeout 20 ms, write timeout 200 ms | `transports.py:26-34` |
| Max frame | 512 B (TU, PC); RM MCU RX/TX buffers 256 B | `tune_config.hpp:12`, `config.hpp:26-31` |
| Lease timeout | 1200 ms both protocols | `tune_config.hpp:26`, `config.hpp:33` |
| PC heartbeat | 400 ms (500 wireless), 700 ms while waiting for MCU; per-request timeout 150 ms | `qt_backend.py:536-537, 694, 847, 860` |
| Request timeouts | 200 ms default, 800 ms HELLO, 2 s SAVE/RESET | `client.py:262, 271, 320, 326` |
| Min inter-request gap | 2 ms | `client.py:220` |
| Status poll | 1500 ms (2500 wireless); runtime poll 500 ms (1000 wireless) | `qt_backend.py:90, 143, 671-672` |
| Catalog page sizes | TU ≤8 (PC asks 2); watch ≤10 (PC asks 2, 2 retries); trace ≤4; tasks ≤7 | `service.cpp:163`, `monitor_service.cpp:132, 285, 341`, `client.py:293, 371` |
| Max active watches | 12; budget 80 000 B/s; sample record 10 B; frame overhead 15 B | `config.hpp:20-37`, `watch.cpp:178-186` |
| Trace ring | 48 events, ≤8 per frame, 13 B each | `config.hpp:23`, `monitor_service.cpp:454` |
| PC series ring | 24 000 points/path; async queue 4096 frames; display ≤30 Hz, 8 paths | `qt_backend.py:114, 119, 288`, `client.py:43` |
| Render | latest cards 100 ms; scope RAF 33/40/50 ms | `app.js:153-154, 245` |
| Params / snapshot | 128 params, 2304 B snapshot, header 20 B, record 16 B | `tune_config.hpp`, `config_store.hpp:47-48` |
| String limits | 255 (TU), 63 (RM), task name 15 | `service.cpp:40-50`, `monitor_service.cpp:34-40`, `runtime.hpp:26` |

## Reuse verdict for tuning-tools

Target: Rust/Tauri host, Rust/embassy firmware, alloc-free (see memory note "No heap"). Descriptors must be `static`s.

- **Adopt (protocol ideas).** One frame codec `magic | ver | cmd | seq u16 | len u16 | payload | crc16` with reply = `cmd | 0x80` and `status u8` first in every reply; CRC-16/MCRF4XX is fine but pick it deliberately (any table-free CRC works). Stable 32-bit FNV-1a ids from `component.name` so ids survive reordering. Catalog paging with `total, returned` and server-side clamping. Fixed 8-byte value slot with a type tag. Slot-based subscriptions with per-variable requested/actual Hz, `recommended_hz`/`max_hz` in the catalog, a byte-rate budget with an explicit `BUDGET` error, batched `SAMPLE` frames with a device timestamp, and a `GET_STATS` counter set (dropped frames is the key one). Independent leases for tune and monitor, heartbeat with rollback-on-timeout, `SESSION_REQUIRED` for everything else. Three-layer values (Default/Saved/Current) with `READ` returning current+saved. Policy enforcement on the MCU against a vehicle-state enum, with the distinct error codes `READ_ONLY`/`RANGE`/`STATE_DENIED`/`STEP_TOO_LARGE`/`VALIDATION`. A/B slot with generation + CRC + read-back verify and per-param factory fingerprint migration. Apply-as-pending-flag: in embassy, an `apply` hook can set an `AtomicBool` or signal a `Watch`/`Signal` that the owning task polls; never touch motors from the tune task.
- **Rust shape.** A `no_std` crate shared by firmware and host: `ParamDescriptor { id, component: &'static str, name, unit, ty, policy, min, max, step, max_step, ptr: *const AtomicU32/AtomicI32/… or a `&'static dyn ParamCell`, factory, apply: Option<fn()> }` as `static` items collected via `#[link_section]` + `linkme::distributed_slice` (or a `build.rs`-generated table when linker tricks are unwanted). Values as atomics avoid the volatile byte-copy hack and give the control task a lock-free read. Host side reuses the same enums and codec in Tauri's Rust backend; the web front end only sees decoded structs.
- **Simplify.** Merge TU/v2 and RM/v1 into one magic/version with command ranges (0x0x tune, 0x1x-0x2x monitor, 0x4x async) so the host demuxes one stream and the MCU keeps one parser and one TX queue. Drop the RTOS task snapshot (embassy has no task list); replace with per-task loop-period/overrun counters exposed as ordinary watches. Drop the 13-byte legacy trace record. Consider making trace ids ordinary watch ids. Widen types only if needed (F64 unnecessary on Cortex-M4F). Keep `max_step` optional. Move the PC-side envelope downsampling and 100 ms/RAF cadence into the Tauri front end as-is in spirit.
- **Do not copy (GPL/LGPL).** No Python, C++, JS, CSS or HTML from either repo; no UI strings, no mock transport, no test vectors verbatim. Re-derive byte layouts from this note, write our own codec and tests, and keep the implementation history clean of pasted snippets. If protocol compatibility with RM Studio PC is ever wanted, the tables above are sufficient; interoperability is not a licensing dependency.
- **Open questions for our design.** Whether one lease should cover both planes (simpler) or two (RM Studio's isolation argument: a stalled tune write should not kill streaming). Whether to add a `GET_VALUES` bulk read to avoid 128 round trips at 200 ms each on connect. Whether to persist to RM-C internal flash sectors (large erase granularity) or an external EEPROM; the 2 KB snapshot fits either.
