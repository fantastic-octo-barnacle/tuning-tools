# FRAME — tuning-tools

Status: draft, 2026-09-17. This is the framing document: what the tool is,
the model it is built on, the constraints it must respect, and the order it
gets built in. Decisions that later become irreversible get their own file in
`docs/adr/`. Architecture notes on the four references are in
`docs/references/` and are cited here as `[dv]` datavis-rs, `[hk]`
herkules-tools, `[mr]` MemRW3, `[rs]` RM Studio.

## 1. What it is

A desktop studio for RoboMaster firmware that does three things in one probe
or serial session:

1. **Watch** — plot and tabulate live variables from the robot at control-loop
   rates, with target-side timestamps when the firmware cooperates.
2. **Tune** — change parameters on the running robot, with the safety check
   made by the firmware, not the host, and with a clear Default / Saved /
   Current lifecycle.
3. **Log** — decode the firmware's defmt stream in the same session, so the
   user never has to choose between logs and plots.

It replaces `herkules-tools` (Tauri shell, read-only, stalled) and
`datavis-rs` (egui, SWD-only, mature engine). Target firmware is
`rm-embedded-rs`: Rust, embassy, `no_std`, alloc-free, defmt over RTT, on
DM-MC02 (STM32H723) and RM-C (STM32F407).

### Non-goals

- Not a debugger. No breakpoints, stepping, or halting from this tool. MemRW3
  shows that is a second product `[mr]`; probe-rs VS Code covers it for us.
- Not a vision or simulator UI. The operator client is `custom-app-27`.
- Not a general serial terminal. A raw byte view exists for diagnosis only.
- Not a browser app. Desktop only; the hosted/WebSerial path of herkules is
  dropped `[hk]`.

## 2. The model

Two layers, kept strictly apart. The UI only ever sees the upper one.

```
             ┌──────────────────────── Studio (Rust) ────────────────────────┐
  UI ◄──────►│  Catalog · SampleStream · TuneApi · LogStream · Status        │
             │        ▲                    ▲                                 │
             │   MemPollSource        TelemetrySource         DefmtLog        │
             │        │                    │                     │            │
             │   MemoryAccess          ByteStream           ByteStream       │
             │   ─────────────        ─────────────────    ─────────────      │
             │   probe-rs MEM-AP      RTT up1/down0        RTT up0           │
             │   (OpenOCD TCL)        USB CDC · UART                          │
             │                        WebSocket · mock                       │
             └───────────────────────────────────────────────────────────────┘
```

**Carriers** move bytes or words and know nothing about variables:

| Carrier | Trait | Notes |
|---|---|---|
| probe-rs session | `MemoryAccess` (read/write words and blocks) | non-halting AP0 access, one owner thread `[dv]` `[mr]` |
| RTT channel | `ByteStream` | rides the same probe session; up 0 defmt, up 1 telemetry, down 0 control |
| USB CDC / UART | `ByteStream` | both boards have USB full-speed on the Type-C port; UART needs an adapter |
| WebSocket / mock | `ByteStream` | simulator and UI development without hardware |
| OpenOCD TCL | `MemoryAccess` | kept only because `[hk]` has it; no bulk reads, lowest priority |

**Sources** turn a carrier into the studio API:

| Source | Needs | Catalog from | Timestamps | Writes |
|---|---|---|---|---|
| `MemPollSource` | `MemoryAccess` | ELF/DWARF, or the firmware descriptor table read out of RAM | host clock per batch | raw word store, no MCU check |
| `TelemetrySource` | `ByteStream` | firmware catalog frames | target `micros()` per batch | framed WRITE with MCU policy check and status reply |

The point of the split: SWD and USB are not interchangeable transports under
one backend, they are different sources with different truth about
timestamps, catalog, and write safety. RTT is a byte stream that happens to
ride the probe, so it belongs with USB, not with memory polling.

### One catalog, two discovery paths

The firmware publishes a **descriptor table**: one `static` per watchable or
tunable value, listed in one `static TABLE: Table` (magic `RMTT`, format
version), each with a stable 32-bit id (FNV-1a of `component.name` `[rs]`), name, unit,
type tag, min/max/step, write policy, and a pointer to an atomic cell.

- Over `TelemetrySource` the firmware serves the table in CATALOG pages.
- Over `MemPollSource` the host finds the table by its DWARF type (a struct
  named `Table` with `magic`, `version` and `entries`), decodes the
  descriptors by field name from the ELF file itself (they are initialised
  statics), and polls the cells directly. Before any write it reads the
  target's table and refuses unless it equals the ELF's, so a stale ELF cannot
  aim a write at unrelated memory.

Same ids, same names, same units on both paths, so a saved layout or tune
profile works whichever carrier is plugged in. Raw DWARF variables remain
available on `MemPollSource` as a second, unstable namespace (address may move
per build; re-resolve by full path on ELF change `[mr]` `[dv]`).

Why the firmware needs a table at all: control state in `rm-embedded-rs`
lives in task-owned structs, not statics; the only statics are signals,
buffers and counters. DWARF alone reaches almost nothing worth tuning.

## 3. Firmware side (`rm-telemetry`, shared `no_std` crate)

Lives in the `rm-embedded-rs` workspace so the firmware and the host build the
same codec and enums from one source.

- **Cells.** `AtomicU32` holding `f32`/`i32`/`u32` bits (a `bool` and small
  ints widen). Reads and writes are single-word, so no torn values on either
  carrier and no critical section in the control loop.
- **Tune cells.** The owning task reads the cell each tick, clamps to the
  descriptor's range and slew-limits by `max_step`. A host write therefore
  cannot push an unsafe value even over raw SWD. The optional `apply` hook
  only sets a pending flag or fires a `Signal`; nothing touches a motor from
  the telemetry task `[rs]`.
- **Policy on the MCU.** `Live`, `SafeOnly`, `KillOnly`, `ReadOnly`, checked
  against the robot's own state enum before any framed WRITE, with distinct
  status codes `READ_ONLY`, `RANGE`, `STATE_DENIED`, `WRONG_KIND`,
  `SESSION_REQUIRED`, `BUSY`, `BUDGET`. A step limit is not an error: the
  owner slews toward the request. `KillOnly` is not implemented; `SafeOnly`
  is, with "safe" supplied by the robot (disarmed). The host greys buttons;
  the MCU refuses `[rs]`.
- **Watch publishing.** A telemetry task with subscriptions: per-id requested
  Hz clamped to `max_hz`, a byte-rate budget with an explicit `BUDGET` error,
  batched SAMPLE frames with device timestamp and sequence number, drop
  counters in `GET_STATS`.
- **Three-layer values.** Default (factory in code), Saved (flash A/B slot
  with generation + CRC16 + read-back verify), Current (session). SAVE,
  DISCARD, RESET_FACTORY as explicit commands. RM Studio rolls Current back
  to Saved on heartbeat timeout `[rs]`; we do not (see section 9). As built:
  SAVE and DISCARD (to Default) exist; there is no RESET_FACTORY and no
  "discard to Saved" yet. The crate owns the record format and slot choice,
  the firmware moves the bytes; a saved value is restored as a request at boot.
- **RTT.** Replace `defmt_rtt` with `rtt-target` (`defmt` feature) and one
  `rtt_init!` per binary: up 0 `defmt` 1 KiB NoBlockSkip, up 1 `telemetry`
  4 KiB NoBlockSkip, down 0 `control` 256 B BlockIfFull. `defmt-rtt`
  hard-codes one channel and owns `_SEGGER_RTT`, so it cannot coexist with a
  second stream. NoBlockSkip drops whole frames under host stall; the sequence
  number makes gaps visible. As built: `hal::rtt::init()` in every firmware,
  with defmt 4 KiB, telemetry 4 KiB and control 512 B, all NoBlockSkip (the
  firmware polls control every 5 ms, so the host never needs to block). The
  block lives in a `.rtt` linker section in DTCM on the H7: in D-cached AXI
  SRAM the core's RdOff store flushed a stale descriptor over the WrOff the
  probe had just written, so requests vanished and defmt frames tore. The
  probe session sends only requests there (lease on first request, WRITE,
  DISCARD, SAVE); it keeps sampling and catalog checks on SWD, and falls back
  to cell writes when the channels are absent.
- **USB CDC.** Same frames on the CDC bulk endpoints; the carrier is chosen at
  runtime by whichever link says HELLO first, or both. DM-MC02 enumerates as
  VID `0xc0de` PID `0xcafe`, product `rm-telemetry`; the host lists ports with
  that product first.
- **Printable encoding.** Optional, behind a feature: the same frames as one
  line of ASCII hex or a `key=value` form for a bare terminal. Not required
  for the stream split; RTT channels already separate console from data.

## 4. Wire protocol (own design, derived from `[rs]`, no code copied)

```
magic u8 | ver u8 | cmd u8 | flags u8 | seq u16 | len u16 | payload | crc16
reply cmd = cmd | 0x80, payload starts with status u8
```

One magic for both planes; command ranges `0x0x` session, `0x1x` catalog and
params, `0x2x` watch, `0x4x` async (SAMPLE, EVENT, LOG). CRC-16/MCRF4XX
(table-free). Fixed 8-byte value slot with a type tag. Little-endian
throughout. Paging: `offset u16, limit u8` in, `total u16, returned u8` out.
The codec lives in `rm_telemetry::wire` and its host mirror
`studio_core::wire`; both test the same HELLO vector so they do not drift. A
slot is tag u8, three reserved bytes, bits u32.

| cmd | request | reply after status |
|---|---|---|
| `0x01` HELLO | — | wire version u8, table version u32, entries u16, fingerprint u32, max payload u16, lease ms u16 |
| `0x02` LEASE | token u32 | — |
| `0x03` RELEASE | token u32 | — |
| `0x10` CATALOG | offset u16, limit u8 | total u16, returned u8, then per entry: id u32, kind u8, access u8, default u32, min/max/step f32, name and unit as u8 length + bytes |
| `0x11` READ | count u8, ids u32 | count u8, then per id: id u32, requested u32, applied u32 |
| `0x12` WRITE | token u32, id u32, slot | id u32, requested u32 |
| `0x13` DISCARD | token u32 | values reset u16 |
| `0x14` SAVE | token u32 | generation u32, sent after the flash write verifies |
| `0x20` WATCH | period ms u16, count u8, ids u32 | watched u8 (count or period 0 stops) |
| `0x22` STATS | — | sample frames sent u32, dropped u32, bad frames u32 |
| `0x40` SAMPLE | unsolicited: device time u64 us, count u8, bits u32 each | — |

## 5. Host side

Rust workspace in this repo:

| Crate | Role | Origin |
|---|---|---|
| `studio-dwarf` | ELF symbols + DWARF types, `VariableStatus`, demangle, rebuild diff | port of `herkules-dwarf`, which is the `[dv]` parser |
| `studio-carriers` | `MemoryAccess` and `ByteStream` impls: probe-rs, RTT, serialport, WebSocket, mock | new, `[hk]` serial and TCL framing reused |
| `studio-core` | sources, catalog, read planner (coalesced aligned slots `[dv]` `[mr]`), scheduler with absolute deadlines, ring buffers, decimation, `ProbeStats`, defmt decoding | port of `[dv]` `read_manager`/`probe_stats`, rest new |
| `rm-telemetry` | codec and descriptors, host side of the same crate | shared with firmware |
| `src-tauri` | thin: commands, one hardware-owner thread, binary channel to the webview | new |

Rules:

- **One hardware owner thread** per session, driven by a command channel and
  emitting typed events; every UI action is a command `[dv]` `[mr]`. probe-rs
  `Core` is borrowed per operation, never held across calls.
- **Plan once, poll many.** Read plans are rebuilt on variable-set change,
  not per tick `[dv]`.
- **Data plane is binary.** Samples cross IPC as fixed-interval frames (30 Hz)
  of columnar `Float64Array`/`Float32Array` over a Tauri `Channel` with raw
  bytes, never JSON per sample and never a 25 ms pull `[hk]`. History lives in
  Rust; the webview gets decimated views of the visible window.
- **Status is a stream**, not a polled error slot `[hk]`. Dropped frames,
  achieved rate, link state, halt state are events.
- **Writes are typed** from the descriptor or `TypeDef`, validated on the
  host for UX and on the MCU for truth, with read-back.

## 6. Frontend

React + TypeScript + uPlot, Tailwind, one dockable layout. Widgets are the
unit of composition; each binds to catalog ids, never to addresses.

| Widget | Reads | Writes |
|---|---|---|
| Scope | sample stream, N channels, cursor sync, FFT on the latest contiguous segment `[mr]` | — |
| Watch table | latest values, tree of structs/arrays, per-row refresh rate `[mr]` | inline edit for `Live` cells |
| Tune panel | descriptors: slider/step/drag with apply-on-release, Default/Saved/Current columns, policy badge | WRITE, SAVE, DISCARD |
| Log console | decoded defmt with level filter, timestamps aligned to the sample clock | — |
| Link status | carrier, rate, budget, drops, session lease, halt state | connect/disconnect |
| Raw frames | hex of TX/RX for protocol diagnosis `[rs]` | send one frame |
| Recorder | trigger window before/after a condition `[rs]`, CSV export | — |

Refresh cadence: cards at 10 Hz, scopes on `requestAnimationFrame` with
adaptive back-off `[rs]`. Catalog lists rebuild only on a structure signature
change, and an edit lock keeps a background refresh from clobbering a field
being typed into `[rs]`.

## 7. Persistence

Layouts and profiles are keyed by **symbol path + descriptor id + ELF hash**,
never by address `[dv]`. Files: `*.tuning-layout.json` (widgets, bindings,
carrier prefs), `*.tune-profile.json` (a Current snapshot with the catalog
fingerprint, importable through the MCU checks, exportable as a Rust
`const` block). Schema versioned from day one `[mr]`.

## 8. Build order

Each milestone ends in something usable on a real robot.

| # | Milestone | Done when |
|---|---|---|
| M0 | Scaffold, workspace, CI, `studio-dwarf` ported with its fixtures | `cargo test` green, ELF opens in the app and shows the symbol tree |
| M1 | Probe session + `MemPollSource` on DWARF statics + defmt log on RTT up 0 | Plot a static counter from a stock `rm-embedded-rs` binary while the log scrolls, no firmware change |
| M2 | `rm-telemetry` descriptor crate; firmware converts a few gimbal PID gains and state values; host reads the table over SWD; tune panel writes cells | Tune pitch `kp` live over SWD and see the response on the scope |
| M3 | Done: framed protocol over RTT up 1 / down 0 and USB CDC; `TelemetrySource`; session lease, policy, SAVE/DISCARD, A/B storage. Checked on the DM-MC02: a saved value survives a full power cycle, and with USB and the probe both attached the tool without the lease is refused with "another tool holds the tuning lease" | Same tune session works over the Type-C cable with no probe, and a value survives a power cycle |
| M3a | Done: framed protocol over USB CDC, lease, policy, DISCARD, link session in the app with no ELF | Tune over the Type-C cable with no probe |
| M3b | Done: SAVE to A/B sectors on the board's W25Q64 QSPI flash, restored at boot. External flash does not stall the CPU the way an internal-flash erase would, and a save costs no control ticks | A value survives a power cycle |
| M3c | Done: tuning requests and SAVE over RTT up 1 / down 0 in the probe session, sharing the firmware's server and lease with USB | Same session over the probe |
| M4 | Recorder, FFT, profiles, layouts, packaging for macOS and Windows | A teammate installs a release and tunes without reading source |
| M5 | Printable encoding, WebSocket carrier for the simulator, OpenOCD fallback | Optional, only if pulled |

## 9. Decisions with a recommendation

- **Leases.** One session covering both planes. RM Studio's two-lease
  isolation `[rs]` buys little once frames carry a sequence number. Settled in
  M3: a lapsed lease does not roll requests back. With no flash there is
  nothing to return to but defaults, and a cable pulled mid-tune should not
  change gains under a running robot. DISCARD is the explicit reset.
- **Descriptor collection.** Settled in M2: one hand-listed `Table` static.
  `linkme` needs `unsafe` link sections, which `#![forbid(unsafe_code)]` in
  the robot crates rules out, and one list per firmware is short enough to
  review. `Table::validate` at boot rejects duplicate ids and keeps the table
  linked.
- **F64.** Not in the wire format. Cortex-M4F/M7 firmware has no use for it;
  the host widens.
- **OpenOCD.** Keep the carrier stub, do not spend on it.
- **Probe read speed.** probe-rs spends two USB exchanges per region; one
  batched CMSIS-DAP packet per tick is ~10× faster for spread-out watches and
  was proven feasible by handing the probe over after a probe-rs attach.
  Deferred; see [ADR 0001](docs/adr/0001-batched-cmsis-dap-reads.md).
- **Simulator.** `rm-sim-rs` should speak the same frames over WebSocket, so
  the studio is also the simulator's tuning UI. Deferred to M5.

## 10. Licence rule

- `datavis-rs` (MIT) and `herkules-tools` (ours): copy freely.
- `MemRW3`: no licence file; the README's one word is not a grant. Designs
  only, no code.
- `rm-studio-pc` (GPL-3) and `rm-studio-mcu` (LGPL-3): wire layouts and ideas
  only, re-derived from `docs/references/rm-studio.md`. No code, strings,
  test vectors or assets.
