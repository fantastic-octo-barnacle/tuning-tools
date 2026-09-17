import { Channel, invoke } from "@tauri-apps/api/core";
import { NodeRef, SymbolNode } from "../elf/api";

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
  | { state: "attached"; channel: string };

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

export type SessionEvent =
  | { type: "status"; state: LinkState; message: string | null }
  | ({ type: "stats" } & Stats)
  | { type: "log"; lines: LogLine[] };

export interface ConnectRequest {
  probe: string | null;
  chip: string;
  speedKhz: number | null;
  rateHz: number;
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

export function setWatches(watches: { id: number; node: NodeRef }[]): Promise<WatchResult[]> {
  return invoke("session_set_watches", { watches });
}

export function setRate(hz: number): Promise<void> {
  return invoke("session_set_rate", { hz });
}

export function watchableLeaves(node: NodeRef): Promise<SymbolNode[]> {
  return invoke("watchable_leaves", { node });
}
