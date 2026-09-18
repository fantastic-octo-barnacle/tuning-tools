import { Channel } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import * as elf from "../elf/api";
import * as live from "../live/api";
import { STANDALONE_STATUS, fixedStatus, localStorageBacked } from "./common";
import type { Host } from "./types";

/** Typed-array views need the frame to start at an 8-byte boundary */
function toAlignedBuffer(message: ArrayBuffer | Uint8Array | number[]): ArrayBuffer {
  if (message instanceof ArrayBuffer) return message;
  if (message instanceof Uint8Array) {
    return message.buffer.slice(message.byteOffset, message.byteOffset + message.byteLength) as ArrayBuffer;
  }
  return new Uint8Array(message).buffer;
}

/** The desktop app: Tauri commands, and channels for the session's streams. */
export const tauriHost: Host = {
  name: "tauri",
  storage: localStorageBacked,
  async startup() {
    return { elfPath: await elf.startupElfPath(), connect: null, connectDefaults: null, watches: [] };
  },
  watchStatus: fixedStatus(STANDALONE_STATUS),
  async pickElf() {
    const path = await open({ title: "Open firmware ELF", multiple: false, directory: false });
    return typeof path === "string" ? path : null;
  },
  openElf: elf.openElf,
  symbolChildren: elf.symbolChildren,
  watchableLeaves: live.watchableLeaves,
  listProbes: live.listProbes,
  listSerialPorts: live.listSerialPorts,
  searchChips: live.searchChips,
  connect(request, handlers) {
    const data = new Channel<ArrayBuffer | Uint8Array | number[]>((frame) => handlers.frame(toAlignedBuffer(frame)));
    const events = new Channel<live.SessionEvent>(handlers.event);
    return live.connect(request, data as Channel<ArrayBuffer>, events);
  },
  disconnect: live.disconnect,
  setRate: live.setRate,
  setWatches: live.setWatches,
  requestValue: live.requestValue,
  saveValues: live.saveValues,
  discardValues: live.discardValues,
  taskStates: live.taskStates,
  readValues: live.readValues,
};
