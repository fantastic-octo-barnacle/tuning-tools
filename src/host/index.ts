import { mockHost } from "./mock";
import { tauriHost } from "./tauri";
import type { Host } from "./types";

export type { Host, HostStartup, SessionHandlers, WatchSeed } from "./types";

/** `?host=mock` forces the simulation; a plain browser tab gets it too, as it has no backend */
function pick(): Host {
  const asked = new URLSearchParams(location.search).get("host");
  if (asked === "mock" || !("__TAURI_INTERNALS__" in window)) return mockHost;
  return tauriHost;
}

export const host: Host = pick();
