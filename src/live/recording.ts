import { invoke } from "@tauri-apps/api/core";

/** A recording in progress, or how one ended (studio-app `RecordingState`) */
export interface RecordingState {
  active: boolean;
  path: string;
  /** Seconds since it started */
  elapsed: number;
  ticks: number;
  /** Bytes on disk */
  bytes: number;
  /** Sample batches lost because writing fell behind */
  dropped: number;
  /** Why it stopped early, if it did */
  error: string | null;
}

/** The live TCP stream (studio-app `StreamState`) */
export interface DataStreamState {
  listening: boolean;
  /** `127.0.0.1:7878` */
  address: string | null;
  bindAll: boolean;
  clients: number;
  /** Sample batches lost over every client */
  dropped: number;
  error: string | null;
}

export interface CsvExport {
  path: string;
  rows: number;
  /** Value columns, besides `time` */
  columns: number;
  /** The recording had no footer (the app was killed); read up to its last chunk */
  truncated: boolean;
}

/** Recording and stream state, over every session */
export type AppEvent = ({ type: "recording" } & RecordingState) | ({ type: "stream" } & DataStreamState);

export interface AppState {
  recording: RecordingState | null;
  stream: DataStreamState;
}

export const DEFAULT_STREAM_PORT = 7878;

export const STREAM_STOPPED: DataStreamState = {
  listening: false,
  address: null,
  bindAll: false,
  clients: 0,
  dropped: 0,
  error: null,
};

/** Tauri event carrying an `AppEvent` */
export const APP_EVENT = "studio-app-event";

export function recordingStart(path: string | null): Promise<RecordingState> {
  return invoke("recording_start", { path });
}

export function recordingStop(): Promise<RecordingState> {
  return invoke("recording_stop");
}

export function exportCsv(mcapPath: string, csvPath: string | null): Promise<CsvExport> {
  return invoke("export_csv", { mcapPath, csvPath });
}

export function streamStart(port: number, bindAll: boolean): Promise<DataStreamState> {
  return invoke("stream_start", { port, bindAll });
}

export function streamStop(): Promise<DataStreamState> {
  return invoke("stream_stop");
}

export function appState(): Promise<AppState> {
  return invoke("app_state");
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export function formatElapsed(seconds: number): string {
  const s = Math.floor(seconds);
  const m = Math.floor(s / 60);
  const h = Math.floor(m / 60);
  const pad = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m % 60)}:${pad(s % 60)}` : `${m}:${pad(s % 60)}`;
}

/** `firmware.mcap` → `firmware.csv` */
export function csvPathFor(mcapPath: string): string {
  return mcapPath.replace(/\.mcap$/i, "") + ".csv";
}
