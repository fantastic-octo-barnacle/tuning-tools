// A simulated chassis-and-gimbal robot for the mock host: a yaw PID on a second-order plant,
// four wheels with a first-order response, and a power buffer. The tuning table's applied
// values drive the model, so tuning changes the traces live.

import type { CatalogEntry } from "../../elf/api";

interface Tunable {
  name: string;
  unit: string;
  default: number;
  min: number;
  max: number;
  step: number;
  /** What the host asked for */
  requested: number;
  /** What the control loop runs with */
  applied: number;
}

function tunable(name: string, unit: string, value: number, min: number, max: number, step: number): Tunable {
  return { name, unit, default: value, min, max, step, requested: value, applied: value };
}

export const tunables: Tunable[] = [
  tunable("gimbal.yaw.kp", "", 14, 0, 40, 0.1),
  tunable("gimbal.yaw.ki", "", 0.4, 0, 5, 0.01),
  tunable("gimbal.yaw.kd", "", 0.35, 0, 2, 0.01),
  tunable("gimbal.yaw.out_max", "A", 3, 0.5, 3, 0.1),
  tunable("chassis.wheel.response", "ms", 60, 20, 300, 5),
  tunable("chassis.max_speed", "rpm", 3000, 0, 8000, 50),
  tunable("power.limit", "W", 80, 40, 120, 1),
];

export const catalogEntries: CatalogEntry[] = tunables.map((t, id) => ({
  id,
  name: t.name,
  unit: t.unit,
  kind: "f32",
  access: "live",
  default: t.default,
  min: t.min,
  max: t.max,
  maxStep: null,
  requestedAddress: 0x2000_0400 + id * 8,
  appliedAddress: 0x2000_0404 + id * 8,
}));

const tv = (name: string) => tunables.find((t) => t.name === name)!.applied;

export const sim = { t: 0, yaw: 0, yawV: 0, yawI: 0, out: 0, w: [0, 0, 0, 0], energy: 60, mode: 1 };

let seed = 1;
/** Deterministic noise in [-0.5, 0.5) */
export function noise() {
  seed = (seed * 16807) % 2147483647;
  return seed / 2147483647 - 0.5;
}

export function yawTarget(t: number) {
  return [0, 0.6, -0.3, 0.45, -0.6, 0.2][Math.floor(t / 3) % 6];
}

function wheelTarget(t: number, i: number) {
  const k = Math.floor((t + i * 0.13) / 2.5);
  return [0, 2600, 2600, -1500, 800, 0][k % 6] * Math.min(1, tv("chassis.max_speed") / 3000) * (i % 2 ? -1 : 1);
}

export function resetSim() {
  Object.assign(sim, { t: 0, yaw: 0, yawV: 0, yawI: 0, out: 0, w: [0, 0, 0, 0], energy: 60, mode: 1 });
}

export function step(dt: number) {
  sim.t += dt;
  // Gimbal: PID on a second-order plant
  const err = yawTarget(sim.t) - sim.yaw;
  sim.yawI = Math.max(-0.3, Math.min(0.3, sim.yawI + err * dt));
  const lim = tv("gimbal.yaw.out_max");
  const pid = tv("gimbal.yaw.kp") * err + tv("gimbal.yaw.ki") * sim.yawI - tv("gimbal.yaw.kd") * sim.yawV * 1.2;
  sim.out = Math.max(-lim, Math.min(lim, pid));
  sim.yawV += (sim.out * 32 - sim.yawV * 1.6) * dt;
  sim.yaw += sim.yawV * dt;
  // Wheels: first-order response
  const tau = tv("chassis.wheel.response") / 1000;
  let draw = 0;
  for (let i = 0; i < 4; i++) {
    const d = wheelTarget(sim.t, i) - sim.w[i];
    sim.w[i] += (d * dt) / tau + noise() * 6;
    draw += Math.abs(d) * 0.012;
  }
  sim.energy = Math.max(0, Math.min(60, sim.energy + (tv("power.limit") - 25 - draw) * dt * 0.35));
  sim.mode = sim.t % 20 < 12 ? 1 : 2;
}

export const MODES = ["Relax", "Following", "Spinning", "Locked"];

/** Readers for the simulated statics, by symbol path */
export const signals: Record<string, () => number> = {
  "chassis::CHASSIS.mode": () => sim.mode,
  "chassis::CHASSIS.wheels[0].speed": () => sim.w[0],
  "chassis::CHASSIS.wheels[0].target": () => wheelTarget(sim.t, 0),
  "chassis::CHASSIS.wheels[1].speed": () => sim.w[1],
  "chassis::CHASSIS.wheels[1].target": () => wheelTarget(sim.t, 1),
  "chassis::CHASSIS.wheels[2].speed": () => sim.w[2],
  "chassis::CHASSIS.wheels[2].target": () => wheelTarget(sim.t, 2),
  "chassis::CHASSIS.wheels[3].speed": () => sim.w[3],
  "chassis::CHASSIS.wheels[3].target": () => wheelTarget(sim.t, 3),
  "gimbal::GIMBAL.yaw.angle": () => sim.yaw,
  "gimbal::GIMBAL.yaw.target": () => yawTarget(sim.t),
  "gimbal::GIMBAL.yaw.pid.integral": () => sim.yawI,
  "gimbal::GIMBAL.yaw.pid.output": () => sim.out,
  "imu::IMU.gyro.x": () => noise() * 0.02,
  "imu::IMU.gyro.y": () => noise() * 0.02,
  "imu::IMU.gyro.z": () => sim.yawV + noise() * 0.08,
  "power::POWER.buffer_energy": () => sim.energy,
  "power::POWER.chassis_power": () =>
    Math.min(tv("power.limit"), Math.abs(sim.w[0] - wheelTarget(sim.t, 0)) * 0.05 + 18 + noise() * 2),
};

/** Values whose reads fail now and then, so the scope shows gaps */
export const flaky = (path: string) => path.includes(".wheels[");

interface MockTask {
  module: string;
  name: string;
  file: string;
  line: number;
  /** Share of the CPU, % */
  cpu: number;
  pollsPerSec: number;
  longestUs: number;
  futureSize: number;
  locals: { name: string; scalar: "f32" | "u32" | "u64"; read: () => number }[];
}

export const tasks: MockTask[] = [
  { module: "gimbal", name: "control_loop", file: "src/gimbal.rs", line: 88, cpu: 9.1, pollsPerSec: 1000, longestUs: 41, futureSize: 212,
    locals: [{ name: "err", scalar: "f32", read: () => yawTarget(sim.t) - sim.yaw }, { name: "ticks", scalar: "u32", read: () => Math.floor(sim.t * 1000) }] },
  { module: "chassis", name: "control_loop", file: "src/chassis.rs", line: 132, cpu: 6.4, pollsPerSec: 1000, longestUs: 38, futureSize: 308,
    locals: [{ name: "draw", scalar: "f32", read: () => 60 - sim.energy }, { name: "ticks", scalar: "u32", read: () => Math.floor(sim.t * 1000) }] },
  { module: "imu", name: "read_task", file: "src/bmi088.rs", line: 57, cpu: 4.2, pollsPerSec: 2000, longestUs: 22, futureSize: 96,
    locals: [{ name: "samples", scalar: "u32", read: () => Math.floor(sim.t * 2000) }] },
  { module: "can", name: "rx_task", file: "src/can.rs", line: 40, cpu: 3.3, pollsPerSec: 4100, longestUs: 12, futureSize: 144,
    locals: [{ name: "frames", scalar: "u32", read: () => Math.floor(sim.t * 4100) }] },
  { module: "referee", name: "uart_task", file: "src/referee.rs", line: 71, cpu: 1.1, pollsPerSec: 210, longestUs: 64, futureSize: 540,
    locals: [{ name: "crc_errors", scalar: "u32", read: () => Math.floor(sim.t / 7) }] },
  { module: "power", name: "monitor", file: "src/power.rs", line: 29, cpu: 0.4, pollsPerSec: 100, longestUs: 9, futureSize: 48,
    locals: [{ name: "energy", scalar: "f32", read: () => sim.energy }] },
  { module: "rm_telemetry", name: "serve", file: "rm_telemetry/src/lib.rs", line: 212, cpu: 0.9, pollsPerSec: 60, longestUs: 118, futureSize: 1180,
    locals: [{ name: "requests", scalar: "u32", read: () => Math.floor(sim.t * 3) }] },
  { module: "main", name: "defmt_flush", file: "src/main.rs", line: 44, cpu: 0.2, pollsPerSec: 200, longestUs: 6, futureSize: 32,
    locals: [{ name: "deadline", scalar: "u64", read: () => Math.floor(sim.t * 1e6) }] },
];
