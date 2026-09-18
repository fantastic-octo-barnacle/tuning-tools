import type { HostStatus, HostStorage } from "./types";

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
};

export function fixedStatus(status: HostStatus) {
  return (listener: (status: HostStatus) => void) => {
    listener(status);
    return () => {};
  };
}
