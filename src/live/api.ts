import { Channel, invoke } from "@tauri-apps/api/core";
import { Catalog, NodeRef, SourceLocation, SymbolNode } from "../elf/api";

export interface ProbeInfo {
  selector: string;
  name: string;
  serial: string | null;
}

export type LinkState = "connecting" | "connected" | "disconnected" | "failed";
export type CoreState = "running" | "halted" | "sleeping" | "lockedUp" | "unknown";
export type StreamState =
  | { state: "absent" }
  | { state: "searching" }
  | { state: "attached"; channel: string; blocking: boolean };

export interface Stats {
  targetHz: number;
  achievedHz: number;
  readAvgUs: number;
  readMaxUs: number;
  jitterUs: number;
  skippedTicks: number;
  failedRegions: number;
  regions: number;
  values: number;
  bytesPerSec: number;
  logBytes: number;
  framesDropped: number;
  core: CoreState;
  log: StreamState;
  lastError: string | null;
}

export interface LogLine {
  hostTime: number;
  timestamp: string | null;
  level: string | null;
  message: string;
  location: string | null;
  module: string | null;
}

export type CatalogCheck =
  | { state: "checking" }
  | { state: "matches" }
  | { state: "differs"; message: string };

export interface TuneValue {
  id: number;
  /** null when the cell could not be read */
  requested: number | null;
  applied: number | null;
}

export type SessionEvent =
  | { type: "status"; state: LinkState; message: string | null }
  | ({ type: "stats" } & Stats)
  | { type: "log"; lines: LogLine[] }
  | { type: "tune"; check: CatalogCheck; values: TuneValue[] }
  | { type: "catalog"; catalog: Catalog };

/** A debug probe on SWD, or the firmware's framed link on a serial port */
export type Carrier = "probe" | "serial";

export interface ConnectRequest {
  carrier: Carrier;
  probe: string | null;
  chip: string;
  speedKhz: number | null;
  port: string | null;
  rateHz: number;
}

export interface PortInfo {
  path: string;
  product: string | null;
  serial: string | null;
  /** The port is a firmware tuning link */
  telemetry: boolean;
}

export function listSerialPorts(): Promise<PortInfo[]> {
  return invoke("list_serial_ports");
}

export function listProbes(): Promise<ProbeInfo[]> {
  return invoke("list_probes");
}

export function searchChips(query: string): Promise<string[]> {
  return invoke("search_chips", { query });
}

export function connect(
  request: ConnectRequest,
  data: Channel<ArrayBuffer>,
  events: Channel<SessionEvent>,
): Promise<void> {
  return invoke("session_connect", { request, data, events });
}

export function disconnect(): Promise<void> {
  return invoke("session_disconnect");
}

export interface WatchResult {
  id: number;
  error: string | null;
}

/** A symbol by `node`, or a tuning table value by `cell` id */
export type WatchTarget = { id: number; node: NodeRef | null; cell: number | null };

export function setWatches(watches: WatchTarget[]): Promise<WatchResult[]> {
  return invoke("session_set_watches", { watches });
}

export function setRate(hz: number): Promise<void> {
  return invoke("session_set_rate", { hz });
}

export function watchableLeaves(node: NodeRef): Promise<SymbolNode[]> {
  return invoke("watchable_leaves", { node });
}

/** Ask the firmware to run tuning value `id` at `value`; rejects with the reason it was not written. */
export function requestValue(id: number, value: number): Promise<void> {
  return invoke("session_request", { id, value });
}

/** Ask the firmware to keep every current value across a power cycle. */
export function saveValues(): Promise<void> {
  return invoke("session_save");
}

/** Ask the firmware for every tunable's built-in default. */
export function discardValues(): Promise<void> {
  return invoke("session_discard");
}

export interface TaskPoint {
  /** `Unresumed`, `Returned`, `Panicked`, or `Suspend0`, `Suspend1`, … */
  label: string;
  /** The future's variant node path */
  path: string;
  /** The `.await` a suspended task is parked on */
  location: SourceLocation | null;
}

export interface TaskState {
  /** The pool slot holds a future */
  spawned: boolean;
  /** Woken, waiting in the run queue to be polled */
  queued: boolean;
  at: TaskPoint | null;
}

export interface TaskStatus {
  /** The task's root node path */
  path: string;
  state: TaskState | null;
  error: string | null;
}

/** Every embassy task's state, read once over the probe. */
export function taskStates(): Promise<TaskStatus[]> {
  return invoke("session_task_states");
}
