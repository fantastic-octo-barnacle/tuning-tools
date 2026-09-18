import type { Children, NodeRef, OpenedElf, SymbolNode } from "../elf/api";
import type {
  ConnectRequest,
  PortInfo,
  ProbeInfo,
  SessionEvent,
  TaskSnapshot,
  ValueRead,
  WatchResult,
  WatchTarget,
} from "../live/api";

/** Where a session's output goes; one pair per `connect` */
export interface SessionHandlers {
  /** A TTS1 sample frame (layout in studio-core `frame.rs`), 8-byte aligned */
  frame: (frame: ArrayBuffer) => void;
  /** Link status, stats, log lines, tuning values and a link's own catalog */
  event: (event: SessionEvent) => void;
}

/** A value to watch when an ELF has no saved watch list */
export interface WatchSeed {
  /** Symbol path, e.g. `gimbal::GIMBAL.yaw.angle` */
  path: string;
  unit?: string;
  plotted?: boolean;
}

/** What the host already knows when the UI starts */
export interface HostStartup {
  /** An ELF to open at once: a command-line argument, a launch configuration */
  elfPath: string | null;
  /** A session to start at once, after the ELF opens */
  connect: ConnectRequest | null;
  /** Connection settings to offer, not connect with, when none are remembered (a launch configuration's chip) */
  connectDefaults: ConnectDefaults | null;
  watches: WatchSeed[];
}

export type ConnectDefaults = Partial<Pick<ConnectRequest, "chip" | "probe" | "speedKhz">>;

/** Whether one way of reaching the target can be used now */
export interface SourceState {
  available: boolean;
  /** Why not; shown on the disabled control */
  reason: string | null;
}

/** The ways to reach the target, as the ConnectBar offers them */
export interface HostSources {
  /** Taking the debug probe for this app alone */
  probe: SourceState;
  /** The robot's USB serial link */
  serial: SourceState;
  /** Sharing the probe with a running debug session */
  debugger: SourceState;
}

/** Something the host did to the session, or offers to do */
export interface HostNotice {
  /** New notices have new ids, so one dismissed stays dismissed */
  id: number;
  message: string;
  /** Offer to connect again with the current settings */
  reconnect: boolean;
}

export interface HostStatus {
  sources: HostSources;
  notice: HostNotice | null;
}

/** Small UI state kept across launches: connection and scope settings, watch lists */
export interface HostStorage {
  get(key: string): string | null;
  set(key: string, value: string): void;
}

/**
 * Everything the UI needs from the backend. Plain async methods and callbacks, so a host
 * can sit on Tauri commands, webview messages, or a simulation.
 */
export interface Host {
  readonly name: "tauri" | "mock" | "vscode";
  readonly storage: HostStorage;
  startup(): Promise<HostStartup>;
  /** Calls `listener` with the current status at once, then on every change; returns an unsubscribe */
  watchStatus(listener: (status: HostStatus) => void): () => void;
  /** Ask the person for an ELF; null when they cancel */
  pickElf(): Promise<string | null>;
  openElf(path: string): Promise<OpenedElf>;
  symbolChildren(node: NodeRef, limit?: number): Promise<Children>;
  watchableLeaves(node: NodeRef): Promise<SymbolNode[]>;

  listProbes(): Promise<ProbeInfo[]>;
  listSerialPorts(): Promise<PortInfo[]>;
  searchChips(query: string): Promise<string[]>;

  /** Resolves once the session starts; its output arrives through `handlers` until the next connect */
  connect(request: ConnectRequest, handlers: SessionHandlers): Promise<void>;
  disconnect(): Promise<void>;
  setRate(hz: number): Promise<void>;
  setWatches(watches: WatchTarget[]): Promise<WatchResult[]>;

  /** Ask the firmware to run tuning value `id` at `value`; rejects with the reason it was not written */
  requestValue(id: number, value: number): Promise<void>;
  /** Keep every current tuning value across a power cycle */
  saveValues(): Promise<void>;
  /** Request every tuning value's built-in default */
  discardValues(): Promise<void>;

  taskStates(): Promise<TaskSnapshot>;
  /** Read each numeric node once, outside the watch list */
  readValues(nodes: NodeRef[]): Promise<ValueRead[]>;
}
