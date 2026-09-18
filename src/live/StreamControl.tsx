import { useCallback, useId, useRef, useState } from "react";
import { host } from "../host";
import { Popover, button, field, primaryButton } from "../ui";
import { DEFAULT_STREAM_PORT } from "./recording";
import { useCapture } from "./useCapture";
import { useHostStatus } from "./useHostStatus";

const SETTINGS_KEY = "stream";

interface Settings {
  port: number;
  bindAll: boolean;
}

function loadSettings(): Settings {
  const fallback = { port: DEFAULT_STREAM_PORT, bindAll: false };
  try {
    return { ...fallback, ...JSON.parse(host.storage.get(SETTINGS_KEY) ?? "{}") };
  } catch {
    return fallback;
  }
}

/** The live TCP stream: a status bar cell that opens its settings */
export function StreamControl() {
  const { stream } = useCapture();
  const { features } = useHostStatus();
  const [open, setOpen] = useState(false);
  const [settings, setSettings] = useState(loadSettings);
  const [portText, setPortText] = useState(String(settings.port));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const anchor = useRef<HTMLButtonElement>(null);
  const close = useCallback(() => setOpen(false), []);
  const id = useId();
  const available = features.stream.available;

  const port = Number(portText);
  const portValid = Number.isInteger(port) && port >= 1 && port <= 65535;
  const clients = `${stream.clients} ${stream.clients === 1 ? "client" : "clients"}`;

  function save(next: Settings) {
    setSettings(next);
    try {
      host.storage.set(SETTINGS_KEY, JSON.stringify(next));
    } catch {
      // Not remembered next launch
    }
  }

  async function toggle() {
    setBusy(true);
    setError(null);
    try {
      if (stream.listening) {
        await host.streamStop();
      } else {
        save({ ...settings, port });
        await host.streamStart(port, settings.bindAll);
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <button
        ref={anchor}
        onClick={() => setOpen((o) => !o)}
        disabled={!available}
        aria-haspopup="dialog"
        aria-expanded={open}
        title={features.stream.reason ?? "Serve live samples over TCP for scripts and other tools"}
        className="flex shrink-0 items-center gap-1.5 border-l border-rule px-3 py-[3px] tabular-nums enabled:hover:bg-sunken enabled:hover:text-ink disabled:opacity-50"
      >
        <span
          aria-hidden
          className={`h-[7px] w-[7px] shrink-0 rounded-full ${stream.listening ? "bg-good" : stream.error ? "bg-danger" : "bg-faint"}`}
        />
        {stream.listening ? `Stream ${stream.address ?? ""} · ${clients}` : "Stream off"}
      </button>
      <Popover anchor={anchor} open={open} onClose={close} place="above-right" label="Live data stream">
        <div className="grid w-[300px] gap-2 p-2">
          <div>
            <div className="text-[13px] font-semibold">Live data stream</div>
            <p className="mt-0.5 leading-snug text-muted">
              Every sample as newline-delimited JSON over TCP, for scripts and other tools. Read-only.
            </p>
          </div>
          <label htmlFor={`${id}-port`} className="flex items-center gap-2">
            <span className="w-10 text-muted">Port</span>
            <input
              id={`${id}-port`}
              type="number"
              min={1}
              max={65535}
              value={portText}
              disabled={stream.listening}
              onChange={(e) => setPortText(e.target.value)}
              className={`${field} w-24 font-mono`}
            />
          </label>
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={settings.bindAll}
              disabled={stream.listening}
              onChange={(e) => save({ ...settings, bindAll: e.target.checked })}
            />
            Allow other machines
          </label>
          {settings.bindAll && (
            <p className="leading-snug text-warn">Anyone on your network can then read the live values; there is no password.</p>
          )}
          {stream.listening && (
            <p className="font-mono tabular-nums">
              listening on {stream.address} · {clients}
              {stream.dropped > 0 && <span className="text-warn"> · {stream.dropped} batches dropped</span>}
            </p>
          )}
          {(error ?? stream.error) && <p className="leading-snug text-danger">{error ?? stream.error}</p>}
          <div className="flex justify-end gap-2">
            <button onClick={close} className={button}>
              Close
            </button>
            <button
              onClick={() => void toggle()}
              disabled={busy || (!stream.listening && !portValid)}
              className={stream.listening ? button : primaryButton}
            >
              {stream.listening ? "Stop" : "Start"}
            </button>
          </div>
        </div>
      </Popover>
    </>
  );
}
