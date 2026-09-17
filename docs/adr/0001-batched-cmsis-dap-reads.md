# 0001 — Batched CMSIS-DAP reads for memory sampling

- **Status:** Proposed, deferred. The prototype works; building it waits until
  sampling speed matters more than features.
- **Date:** 2026-09-17
- **Scope:** `studio-carriers`, the probe carrier behind `Link`

## Context

Memory polling (M1) reads the watched RAM regions over SWD on every tick. How
fast that can go sets the highest usable sample rate and how many values fit in
one tick.

Two costs were found and measured on one setup:

- **Probe:** Horco CMSIS-DAP. CMSIS-DAP v1 over HID on USB high speed, firmware
  2.1.0, 240-byte packets, 4 packets queued, atomic commands supported (DAP_Info
  capabilities `13 01`).
- **Target:** DM-MC02 (STM32H723VG), macOS host.

### 1. Opening a memory handle per read (fixed)

`Session::core(0)` and `ArmDebugInterface::memory_interface()` each build a
`MemoryAp`. That reads IDR, CSW and CFG and writes CSW every time, which is
several USB round trips. Doing it once per read capped a 1000 Hz target at
~125 Hz, with 16-byte reads taking ~4.4 ms.

The session now opens core 0's MEM-AP once and keeps it while sampling
(`Link::with_memory`). RTT and the core state (DHCSR) are read through the same
handle. The same target now reaches ~760 Hz, with 16-byte reads at ~1.2 ms.

### 2. One USB exchange per read (this record)

The probe-rs CMSIS-DAP driver sends a packet as soon as a read is queued
(`probe/cmsisdap/mod.rs`, `batch_add`). A packet therefore never holds more than
one read. A block read is sent as its own `DAP_TransferBlock` after the pending
TAR write goes out, so every region costs two USB exchanges.

CMSIS-DAP can do far better. A single `DAP_Transfer` can carry many writes and
reads, for example `TAR=a, DRW×n, TAR=b, DRW×m`. `DAP_ExecuteCommands` can
combine several commands into one packet.

## Measurements

Every exchange with this probe has a floor of about **0.5 ms**, even `DAP_Info`,
which never touches the target. SWD wire time adds to that; it matters at 1–4
MHz and mostly disappears from 10 MHz up.

Held MEM-AP handle through probe-rs (current app path):

| SWD clock | 4 B | 16 B | 64 B | 256 B |
|---|---|---|---|---|
| 4 MHz | 521 µs | 1155 µs | 1594 µs | 3489 µs |
| 10 MHz | 505 µs | 1024 µs | 1015 µs | 1900 µs |

Raw CMSIS-DAP, everything in one packet:

| SWD clock | TAR + 16 words (64 B) | 2 regions × 16 B | 4 regions × 16 B | 8 regions × 16 B |
|---|---|---|---|---|
| 1 MHz | 2503 µs | 1888 µs | 3190 µs | 5821 µs |
| 4 MHz | 1112 µs | 948 µs | 1431 µs | 2185 µs |
| 10 MHz | 523 µs | 571 µs | 680 µs | 816 µs |
| 20 MHz | 735 µs | 741 µs | 775 µs | 1079 µs |

At 10 MHz, the probe-rs two-exchange pattern (TAR write, then TransferBlock)
took 979 µs for 16 B and 1450 µs for 256 B.

Summary at 10 MHz:

| Watched set | probe-rs today | One packet |
|---|---|---|
| 1 region, 16 B | ~1.0 ms | ~0.52 ms |
| 8 regions × 16 B | ~8 ms | ~0.82 ms |

Sustained sampling with the raw driver, using one packet per tick that includes
DHCSR, ran at 1844–1912 Hz with no failed transfers.

## Handover from probe-rs

probe-rs knows each chip's attach sequences (the H7 needs DBGMCU setup), so it
keeps doing the attach. Only sampling moves to the raw driver.

Tested in a single process:

1. **probe-rs attaches** (`Probe::attach`, ~85 ms).
2. **Drop the `Session`** (~43 ms). probe-rs clears hardware breakpoints and
   runs `debug_core_stop`, which writes DHCSR without C_DEBUGEN and sets DEMCR to
   0. It then runs `debug_port_stop` (CTRL/STAT = 0) and `DAP_Disconnect`.
   None of this halts or resets the core. `DBGMCU_CR` stayed `0x0070003f`.
3. **Open the probe's HID interface with `hidapi`** (~12 ms). The hidapi context
   is reference-counted, so it can coexist with probe-rs. Then connect the raw
   driver:
   - `DAP_Connect(SWD)`, `DAP_SWJ_Clock`, `DAP_TransferConfigure`,
     `DAP_SWD_Configure`
   - line reset, JTAG-to-SWD sequence, line reset, idle
   - read DPIDR
   - write ABORT `0x1e` to clear sticky errors, SELECT 0, CTRL/STAT
     `0x50000000`, then check that both power-up ACK bits are set
   - on AP0, read CSW and write it back with Size = 32-bit and AddrInc = single
4. **Sample.**
5. **Hand back.** Send `DAP_Disconnect`, close HID; probe-rs attaches normally
   (~85 ms).

Firmware tested:

| Firmware | Core | Result |
|---|---|---|
| `balance-infantry-chassis` (parked in a boot panic) | running | 1844 Hz, 2 regions per packet, 0 failures |
| `dm-mc02-led-test` (embassy, idles in WFI) | asleep in 9561 of 9562 samples | 1912 Hz, 3 regions per packet, 0 failures |

With the LED test, TIM2 CNT (the embassy time driver) changed on every sample,
and the RTT up-0 write offset advanced 10 times in 5 s (the test logs twice a
second). Reads stay live while the core sleeps.

## Decision (proposed)

When sampling speed becomes a priority:

- **CMSIS-DAP fast path.** Add a CMSIS-DAP-only carrier behind `Link`. probe-rs
  attaches, the session is dropped, and the raw driver serves `with_memory`.
- **Batched plan.** The read plan becomes one batched request per tick: a TAR
  write plus DRW reads per region, packed into as few packets as the probe's
  packet size allows. Queued packets (the probe's packet count) can be sent
  before reading any responses. Region boundaries must also respect the
  1 KB auto-increment limit, and reads must be word-aligned.
- **Fallback.** All other probes (ST-Link, J-Link, CMSIS-DAP that fails
  handover) keep the current probe-rs path.
- **SWD clock.** Raise the default from 4 MHz to 10 MHz, keeping the picker for
  poor wiring.

Also worth taking from datavis-rs at the same time: sample plotted values at the
full rate and table-only values at a low rate, so the per-tick packet stays
small.

## Consequences and open points

- **Transports.** v1 (HID) and v2 (bulk, `nusb`) CMSIS-DAP need two transports.
  Only v1 has been measured.
- **Packet size.** A 240-byte packet returns about 58 words per exchange, so
  larger watch sets span several packets or use queued packets.
- **Firmware reset.** Recovery after a target reset or power loss (re-power the
  DP, restore CSW) must be handled by the raw driver, since probe-rs no longer
  owns the link.
- **Not measured:** Linux or Windows HID behaviour, CMSIS-DAP v2 probes, and
  cold attach where probe-rs never set up DBGMCU. The last case doesn't arise
  if probe-rs always attaches first.
- **Timestamps.** Host-polled samples still carry ±0.5 ms of host timing
  jitter. Sample-exact data from a 1 kHz control loop needs the firmware to
  stream its own timestamped samples (M3), not faster polling.
