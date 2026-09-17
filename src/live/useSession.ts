import { useCallback, useRef, useState } from "react";
import { Channel } from "@tauri-apps/api/core";
import * as api from "./api";
import { samples } from "./samples";

const MAX_LOG_LINES = 5000;

export interface Link {
  state: api.LinkState | "idle";
  message: string | null;
}

export interface Tune {
  check: api.CatalogCheck;
  values: Map<number, api.TuneValue>;
}

export function useSession() {
  const [link, setLink] = useState<Link>({ state: "idle", message: null });
  const [stats, setStats] = useState<api.Stats | null>(null);
  const [logs, setLogs] = useState<api.LogLine[]>([]);
  const [tune, setTune] = useState<Tune | null>(null);
  // Events from a replaced connection are ignored
  const generation = useRef(0);

  const connect = useCallback(async (request: api.ConnectRequest) => {
    const gen = ++generation.current;
    const live = () => gen === generation.current;
    samples.clear();
    setStats(null);
    setLogs([]);
    setTune(null);
    setLink({ state: "connecting", message: null });

    const data = new Channel<ArrayBuffer>((frame) => {
      if (live()) samples.ingest(frame);
    });
    const events = new Channel<api.SessionEvent>((event) => {
      if (!live()) return;
      if (event.type === "status") {
        setLink({ state: event.state, message: event.message });
        if (event.state !== "connected") {
          setStats(null);
          setTune(null);
        }
      } else if (event.type === "tune") {
        setTune({ check: event.check, values: new Map(event.values.map((v) => [v.id, v])) });
      } else if (event.type === "stats") {
        setStats(event);
      } else {
        setLogs((old) => {
          const next = old.concat(event.lines);
          return next.length > MAX_LOG_LINES ? next.slice(next.length - MAX_LOG_LINES) : next;
        });
      }
    });
    try {
      await api.connect(request, data, events);
    } catch (e) {
      if (live()) setLink({ state: "failed", message: String(e) });
    }
  }, []);

  const disconnect = useCallback(async () => {
    await api.disconnect();
    setTune(null);
    // The session reports `disconnected` itself; this covers a session that never started
    setLink((l) => (l.state === "connecting" || l.state === "connected" ? l : { state: "idle", message: null }));
  }, []);

  const clearLogs = useCallback(() => setLogs([]), []);

  return { link, stats, logs, tune, connect, disconnect, clearLogs };
}
