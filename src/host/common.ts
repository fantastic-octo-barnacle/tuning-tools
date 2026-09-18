import type { AppEvent } from "../live/recording";
import type { HostFeatures, HostStatus, HostStorage } from "./types";

/** Browser storage; nothing is remembered when it is unavailable */
export const localStorageBacked: HostStorage = {
  get(key) {
    try {
      return localStorage.getItem(key);
    } catch {
      return null;
    }
  },
  set(key, value) {
    try {
      localStorage.setItem(key, value);
    } catch {
      // Storage full or unavailable: not remembered next launch
    }
  },
};

const AVAILABLE = { available: true, reason: null };

/** A host with a real backend has every feature */
export const ALL_FEATURES: HostFeatures = {
  record: AVAILABLE,
  exportCsv: AVAILABLE,
  stream: AVAILABLE,
  reveal: AVAILABLE,
};

/** A standalone app owns the probe; only VS Code has debug sessions to share it with */
export const STANDALONE_STATUS: HostStatus = {
  sources: {
    probe: { available: true, reason: null },
    serial: { available: true, reason: null },
    debugger: {
      available: false,
      reason: "Share the probe with a running probe-rs debug session; only inside VS Code",
    },
  },
  notice: null,
  features: ALL_FEATURES,
};

export function fixedStatus(status: HostStatus) {
  return (listener: (status: HostStatus) => void) => {
    listener(status);
    return () => {};
  };
}

/** App events fan out to every listener */
export function appEventHub() {
  const listeners = new Set<(event: AppEvent) => void>();
  return {
    emit(event: AppEvent) {
      listeners.forEach((l) => l(event));
    },
    watch(listener: (event: AppEvent) => void) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}
