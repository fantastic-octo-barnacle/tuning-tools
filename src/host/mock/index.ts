// A host with no hardware: the simulated robot in ./firmware behind the same interface the
// desktop app uses. It emits TTS1 frames and session events the way the Tauri backend does.

import type { NodeRef } from "../../elf/api";
import type { ConnectRequest, LogLine, SessionEvent, TaskSnapshot, WatchTarget } from "../../live/api";
import { STANDALONE_STATUS, fixedStatus, localStorageBacked } from "../common";
import type { Host, SessionHandlers } from "../types";
import { MODES, catalogEntries, flaky, noise, resetSim, sim, step, tasks, tunables, yawTarget } from "./firmware";
import { MOCK_ELF_PATH, childrenOf, leavesOf, mockElf, nodeAt, readerFor } from "./elf";

const MAGIC = 0x31535454;
const CHIPS = ["STM32F407IGHx", "STM32F427IIHx", "STM32H723VGTx", "STM32H723ZGTx", "STM32H743IITx", "STM32G474RETx"];
/** Cycle counter rate of the simulated core */
const CLOCK_HZ = 480_000_000;

interface Column {
  id: number;
  read: () => number;
  flaky: boolean;
}

interface Session {
  request: ConnectRequest;
  handlers: SessionHandlers;
  timers: ReturnType<typeof setInterval>[];
  rateHz: number;
  lastWall: number;
  owed: number;
  ticks: { n: number; since: number; hz: number };
  /** Task counters, advanced on every read */
  counters: { polls: number; cycles: number }[];
  lastTaskRead: number;
  lastMode: number;
  lowPower: boolean;
}

let session: Session | null = null;
let columns: Column[] = [];
let watched: WatchTarget[] = [];

function emit(event: SessionEvent) {
  session?.handlers.event(event);
}

function log(level: string, module: string, message: string) {
  const line: LogLine = {
    hostTime: sim.t,
    timestamp: sim.t.toFixed(3),
    level,
    message,
    location: null,
    module,
  };
  emit({ type: "log", lines: [line] });
}

/** Encode ticks as a TTS1 frame: header, times, then one column per watch */
function frame(times: number[], values: Float64Array[]): ArrayBuffer {
  const n = times.length;
  const buf = new ArrayBuffer(16 + 8 * n + values.length * (8 + 8 * n));
  new Uint32Array(buf, 0, 4).set([MAGIC, n, values.length, 0]);
  new Float64Array(buf, 16, n).set(times);
  let at = 16 + 8 * n;
  values.forEach((column, c) => {
    new Uint32Array(buf, at, 2).set([columns[c].id, 0]);
    new Float64Array(buf, at + 8, n).set(column);
    at += 8 + 8 * n;
  });
  return buf;
}

/** Run the ticks owed since the last call and send them as one frame, as the session thread does */
function pump(s: Session) {
  const now = performance.now();
  s.owed += (Math.min(1000, now - s.lastWall) / 1000) * s.rateHz;
  s.lastWall = now;
  const n = Math.floor(s.owed);
  s.owed -= n;
  if (n === 0) return;
  const dt = 1 / s.rateHz;
  const times: number[] = [];
  const values = columns.map(() => new Float64Array(n));
  for (let i = 0; i < n; i++) {
    step(dt);
    for (const t of tunables) t.applied = t.requested;
    times.push(sim.t);
    columns.forEach((c, k) => {
      values[k][i] = c.flaky && noise() > 0.49995 ? NaN : c.read();
    });
  }
  s.ticks.n += n;
  if (now - s.ticks.since > 500) {
    s.ticks.hz = (s.ticks.n * 1000) / (now - s.ticks.since);
    s.ticks.n = 0;
    s.ticks.since = now;
  }
  s.handlers.frame(frame(times, values));
}

function sendStats(s: Session) {
  const values = columns.length;
  const regions = Math.max(1, Math.ceil(values / 3));
  const probe = s.request.carrier === "probe";
  emit({
    type: "stats",
    targetHz: s.rateHz,
    achievedHz: s.ticks.hz,
    readAvgUs: values ? (probe ? 180 + regions * 140 : 90) : 0,
    readMaxUs: values ? (probe ? 420 + regions * 160 : 240) : 0,
    jitterUs: 35,
    skippedTicks: 0,
    failedRegions: 0,
    regions,
    values,
    bytesPerSec: values * 4 * s.rateHz,
    logBytes: 0,
    framesDropped: 0,
    core: "running",
    log: probe ? { state: "attached", channel: "defmt", blocking: false } : { state: "absent" },
    lastError: null,
  });
}

function sendTune() {
  emit({
    type: "tune",
    check: { state: "matches" },
    values: tunables.map((t, id) => ({ id, requested: t.requested, applied: t.applied })),
  });
}

/** The firmware's own chatter, on the defmt channel */
function chatter(s: Session) {
  if (s.request.carrier !== "probe") return;
  if (sim.mode !== s.lastMode) {
    log("info", "chassis", `mode ${MODES[s.lastMode]} -> ${MODES[sim.mode]}`);
    s.lastMode = sim.mode;
  }
  const low = sim.energy < 20;
  if (low && !s.lowPower) log("warn", "power", `buffer at ${sim.energy.toFixed(1)} J, limiting chassis`);
  s.lowPower = low;
  if (noise() > 0.3) log("debug", "gimbal", `yaw err ${(yawTarget(sim.t) - sim.yaw).toFixed(4)} rad, out ${sim.out.toFixed(3)} A`);
  if (noise() > 0.47) log("warn", "can", "bus 2: tx mailbox full, dropped 1 frame (motor 0x205)");
  if (noise() > 0.495) log("error", "referee", "frame crc16 mismatch (cmd 0x0201), resyncing");
  if (noise() > 0.45) log("trace", "imu", `fifo drained, ${Math.floor(20 + noise() * 8)} samples`);
}

function resolve(targets: WatchTarget[]) {
  const results = targets.map((t) => {
    if (t.cell !== null) {
      const tunable = tunables[t.cell];
      return tunable
        ? { id: t.id, error: null, read: () => tunable.applied, flaky: false }
        : { id: t.id, error: "The tuning table has no such value" };
    }
    const node = t.node && nodeAt(t.node);
    if (!node) return { id: t.id, error: "Not in this ELF" };
    if (node.kind === "pointer") return { id: t.id, error: "Not a number that can be sampled (a pointer)" };
    const read = readerFor(node);
    if (!read) return { id: t.id, error: "Not a number that can be sampled" };
    return { id: t.id, error: null, read, flaky: flaky(node.path) };
  });
  columns = results.flatMap((r) => ("read" in r && r.read ? [{ id: r.id, read: r.read, flaky: r.flaky }] : []));
  return results.map(({ id, error }) => ({ id, error }));
}

function requireSession(what: string): Session {
  if (!session) throw new Error(`Connect to ${what}`);
  return session;
}

function stop() {
  if (!session) return;
  session.timers.forEach(clearInterval);
  session = null;
}

const delay = (ms: number) => new Promise((r) => setTimeout(r, ms));

export const mockHost: Host = {
  name: "mock",
  storage: localStorageBacked,
  watchStatus: fixedStatus(STANDALONE_STATUS),
  async startup() {
    return {
      elfPath: MOCK_ELF_PATH,
      connect: { carrier: "probe", probe: null, chip: "STM32H723VGTx", speedKhz: 10000, port: null, rateHz: 1000 },
      connectDefaults: null,
      watches: [
        { path: "gimbal::GIMBAL.yaw.target", unit: "rad" },
        { path: "gimbal::GIMBAL.yaw.angle", unit: "rad" },
        { path: "gimbal::GIMBAL.yaw.pid.output", unit: "A" },
        { path: "chassis::CHASSIS.wheels[0].target", unit: "rpm" },
        { path: "chassis::CHASSIS.wheels[0].speed", unit: "rpm" },
        { path: "power::POWER.buffer_energy", unit: "J" },
        { path: "chassis::CHASSIS.mode", plotted: false },
      ],
    };
  },
  async pickElf() {
    return MOCK_ELF_PATH;
  },
  async openElf(path) {
    await delay(120);
    if (path !== MOCK_ELF_PATH) throw new Error("The mock host only has its simulated firmware");
    return mockElf;
  },
  async symbolChildren(node: NodeRef) {
    const nodes = childrenOf(node);
    return { nodes, total: nodes.length };
  },
  async watchableLeaves(node) {
    const found = nodeAt(node);
    if (!found) throw new Error("Not in this ELF");
    return leavesOf(found);
  },
  async listProbes() {
    return [
      { selector: "c251:f001:0A1B2C", name: "Horco CMSIS-DAP", serial: "0A1B2C" },
      { selector: "0483:3754:003F", name: "STLINK-V3", serial: "003F" },
    ];
  },
  async listSerialPorts() {
    return [{ path: "/dev/cu.usbmodem101", product: "RM Telemetry", serial: "5A3C", telemetry: true }];
  },
  async searchChips(query) {
    const q = query.toLowerCase();
    return CHIPS.filter((c) => c.toLowerCase().includes(q));
  },

  async connect(request, handlers) {
    stop();
    if (request.carrier === "probe" && !request.chip) throw new Error("Enter the target chip");
    const now = performance.now();
    const s: Session = {
      request,
      handlers,
      timers: [],
      rateHz: request.rateHz,
      lastWall: now,
      owed: 0,
      ticks: { n: 0, since: now, hz: 0 },
      counters: tasks.map(() => ({ polls: 0, cycles: 0 })),
      lastTaskRead: now,
      lastMode: 1,
      lowPower: false,
    };
    session = s;
    resetSim();
    handlers.event({ type: "status", state: "connecting", message: null });
    await delay(150);
    if (session !== s) return;
    handlers.event({ type: "status", state: "connected", message: null });
    if (request.carrier === "serial") handlers.event({ type: "catalog", catalog: mockElf.catalog! });
    resolve(watched);
    if (request.carrier === "probe") {
      log("info", "host", `attached ${request.chip} at ${(request.speedKhz ?? 4000) / 1000} MHz SWD, no halt`);
      log("info", "defmt", "RTT channel 0 'defmt' attached, non-blocking");
    }
    handlers.event({ type: "tune", check: { state: "checking" }, values: [] });
    s.timers.push(
      // The backend sends frames about 30 times a second
      setInterval(() => pump(s), 33),
      setInterval(() => sendStats(s), 500),
      setInterval(sendTune, 250),
      setInterval(() => chatter(s), 400),
    );
  },
  async disconnect() {
    const handlers = session?.handlers;
    stop();
    handlers?.event({ type: "status", state: "disconnected", message: null });
  },
  async setRate(hz) {
    if (session) session.rateHz = hz;
  },
  async setWatches(targets) {
    watched = targets;
    return resolve(targets);
  },

  async requestValue(id, value) {
    requireSession("change values");
    const t = tunables[id];
    if (!t) throw new Error("The tuning table has no such value");
    // The firmware applies the request inside its declared range and step
    const clamped = Math.round(Math.min(t.max, Math.max(t.min, value)) / t.step) * t.step;
    t.requested = Number(clamped.toFixed(4));
    log("info", "telemetry", `set ${t.name} = ${t.requested}`);
    sendTune();
  },
  async saveValues() {
    requireSession("save values");
    await delay(200);
    log("info", "telemetry", "saved tuning table to flash (sector 7)");
  },
  async discardValues() {
    requireSession("reset values");
    catalogEntries.forEach((e) => (tunables[e.id].requested = e.default));
    sendTune();
  },

  async taskStates(): Promise<TaskSnapshot> {
    const s = requireSession("read tasks");
    const now = performance.now();
    const dt = (now - s.lastTaskRead) / 1000;
    s.lastTaskRead = now;
    const queuedAt = Math.floor(now / 300) % tasks.length;
    return {
      hostUs: now * 1000,
      hasStats: true,
      clockHz: CLOCK_HZ,
      untracked: 0,
      statsError: null,
      tasks: mockElf.tasks.map((task, i) => {
        const spec = tasks[i];
        const c = s.counters[i];
        const jitter = 0.9 + (noise() + 0.5) * 0.2;
        c.polls = (c.polls + Math.round(spec.pollsPerSec * dt)) >>> 0;
        c.cycles = (c.cycles + Math.round((spec.cpu / 100) * jitter * CLOCK_HZ * dt)) >>> 0;
        const longest = (spec.longestUs * CLOCK_HZ) / 1e6;
        return {
          path: task.root.path,
          state: { spawned: true, queued: i === queuedAt, at: task.states[1] },
          error: null,
          counters: { polls: c.polls, cycles: c.cycles, maxCycles: Math.round(longest * 1.4), recentMaxCycles: Math.round(longest * jitter) },
        };
      }),
    };
  },
  async readValues(refs) {
    return refs.map((ref) => {
      const node = nodeAt(ref);
      const read = node && readerFor(node);
      return read ? { value: read(), error: null } : { value: null, error: "Not a number that can be read" };
    });
  },
};
