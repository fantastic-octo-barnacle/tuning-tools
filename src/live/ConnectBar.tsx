import { useEffect, useId, useState } from "react";
import * as api from "./api";
import { Link } from "./useSession";

const RATES = [10, 50, 100, 200, 500, 1000];
/** SWD clock; faster makes large reads quicker but needs short, clean wiring */
const SPEEDS = [1000, 4000, 10000];
const SETTINGS_KEY = "connection";

interface Settings {
  carrier: api.Carrier;
  port: string;
  probe: string;
  chip: string;
  rateHz: number;
  speedKhz: number;
}

function loadSettings(): Settings {
  const fallback: Settings = { carrier: "probe", port: "", probe: "", chip: "", rateHz: 100, speedKhz: 4000 };
  try {
    return { ...fallback, ...JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? "{}") };
  } catch {
    return fallback;
  }
}

interface Props {
  link: Link;
  /** An ELF is open, which the probe needs */
  canConnect: boolean;
  onConnect: (request: api.ConnectRequest) => void;
  onDisconnect: () => void;
}

export function ConnectBar({ link, canConnect, onConnect, onDisconnect }: Props) {
  const [settings, setSettings] = useState(loadSettings);
  const [probes, setProbes] = useState<api.ProbeInfo[] | null>(null);
  const [chips, setChips] = useState<string[]>([]);
  const [ports, setPorts] = useState<api.PortInfo[] | null>(null);
  const chipList = useId();
  const active = link.state === "connecting" || link.state === "connected";

  const update = (patch: Partial<Settings>) => {
    const next = { ...settings, ...patch };
    setSettings(next);
    try {
      localStorage.setItem(SETTINGS_KEY, JSON.stringify(next));
    } catch {
      // Not remembered next launch; nothing else depends on it
    }
  };

  const refreshProbes = () => {
    setProbes(null);
    api.listProbes().then(setProbes, () => setProbes([]));
  };
  useEffect(refreshProbes, []);

  const refreshPorts = () => {
    setPorts(null);
    api.listSerialPorts().then(setPorts, () => setPorts([]));
  };
  useEffect(refreshPorts, []);

  useEffect(() => {
    const query = settings.chip.trim();
    if (query.length < 3) return setChips([]);
    let stale = false;
    api.searchChips(query).then((names) => !stale && setChips(names));
    return () => {
      stale = true;
    };
  }, [settings.chip]);

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    if (active) onDisconnect();
    else
      onConnect({
        carrier: settings.carrier,
        probe: settings.probe || null,
        chip: settings.chip.trim(),
        speedKhz: settings.speedKhz,
        port: port || null,
        rateHz: settings.rateHz,
      });
  };

  const serial = settings.carrier === "serial";
  const selectedMissing = settings.probe && probes && !probes.some((p) => p.selector === settings.probe);
  // Fall back to the first firmware port when the remembered one is gone
  const portMissing = settings.port && ports && !ports.some((p) => p.path === settings.port);
  const port = settings.port || ports?.find((p) => p.telemetry)?.path || "";
  const blocked = serial
    ? !port
      ? "Plug in the robot's USB cable, then rescan"
      : null
    : !canConnect
      ? "Open the firmware ELF first"
      : !settings.chip.trim()
        ? "Enter the target chip"
        : null;

  return (
    <form onSubmit={submit} className="ml-auto flex flex-wrap items-center gap-2">
      <div role="radiogroup" aria-label="Connect through" className="flex rounded-sm border border-rule bg-panel p-0.5">
        {(["probe", "serial"] as const).map((c) => (
          <button
            key={c}
            type="button"
            role="radio"
            aria-checked={settings.carrier === c}
            disabled={active}
            onClick={() => update({ carrier: c })}
            title={c === "probe" ? "Debug probe on SWD; needs the ELF" : "The robot's USB cable; no probe or ELF needed"}
            className={`rounded-[1px] px-2 py-0.5 disabled:opacity-60 ${
              settings.carrier === c ? "bg-surface text-ink shadow-[0_0_0_1px_var(--color-rule)]" : "text-muted hover:text-ink"
            }`}
          >
            {c === "probe" ? "Probe" : "USB"}
          </button>
        ))}
      </div>
      {serial ? (
        <>
          <label className="flex items-center gap-1.5">
            <span className="text-muted">Port</span>
            <select
              value={port}
              onChange={(e) => update({ port: e.currentTarget.value })}
              disabled={active}
              className="max-w-60 rounded-sm border border-rule bg-surface px-1 py-1"
            >
              {!port && <option value="">{ports === null ? "Looking for ports…" : "No robot found"}</option>}
              {ports?.map((p) => (
                <option key={p.path} value={p.path}>
                  {p.path.replace(/^\/dev\//, "")}
                  {p.product ? ` (${p.product})` : ""}
                </option>
              ))}
              {portMissing && <option value={settings.port}>{settings.port} (not attached)</option>}
            </select>
          </label>
          <button
            type="button"
            onClick={refreshPorts}
            disabled={active}
            title="Look for serial ports again"
            className="rounded-sm border border-rule bg-panel px-2 py-1 hover:bg-sunken disabled:opacity-60"
          >
            Rescan
          </button>
        </>
      ) : (
        <>
          <label className="flex items-center gap-1.5">
            <span className="text-muted">Probe</span>
            <select
              value={settings.probe}
              onChange={(e) => update({ probe: e.currentTarget.value })}
              disabled={active}
              className="max-w-52 rounded-sm border border-rule bg-surface px-1 py-1"
            >
              <option value="">{probes === null ? "Looking for probes…" : probes.length ? "First one found" : "None found"}</option>
              {probes?.map((p) => (
                <option key={p.selector} value={p.selector}>
                  {p.name}{p.serial ? ` (${p.serial})` : ""}
                </option>
              ))}
              {selectedMissing && <option value={settings.probe}>{settings.probe} (not attached)</option>}
            </select>
          </label>
          <button
            type="button"
            onClick={refreshProbes}
            disabled={active}
            title="Look for probes again"
            className="rounded-sm border border-rule bg-panel px-2 py-1 hover:bg-sunken disabled:opacity-60"
          >
            Rescan
          </button>
          <label className="flex items-center gap-1.5">
            <span className="text-muted">Chip</span>
            <input
              value={settings.chip}
              onChange={(e) => update({ chip: e.currentTarget.value })}
              disabled={active}
              list={chipList}
              placeholder="STM32H723VG"
              spellCheck={false}
              className="w-40 rounded-sm border border-rule bg-surface px-2 py-1 font-mono text-[12px] placeholder:text-muted"
            />
            <datalist id={chipList}>
              {chips.map((c) => (
                <option key={c} value={c} />
              ))}
            </datalist>
          </label>
          <label className="flex items-center gap-1.5" title="SWD clock speed. Lower it if reads fail on long or noisy wiring.">
            <span className="text-muted">SWD</span>
            <select
              value={settings.speedKhz}
              onChange={(e) => update({ speedKhz: Number(e.currentTarget.value) })}
              disabled={active}
              className="rounded-sm border border-rule bg-surface px-1 py-1"
            >
              {SPEEDS.map((k) => (
                <option key={k} value={k}>{k / 1000} MHz</option>
              ))}
            </select>
          </label>
        </>
      )}
      <label className="flex items-center gap-1.5">
        <span className="text-muted">Sample at</span>
        <select
          value={settings.rateHz}
          onChange={(e) => {
            const rateHz = Number(e.currentTarget.value);
            update({ rateHz });
            if (link.state === "connected") void api.setRate(rateHz);
          }}
          className="rounded-sm border border-rule bg-surface px-1 py-1"
        >
          {RATES.map((r) => (
            <option key={r} value={r}>{r} Hz</option>
          ))}
        </select>
      </label>
      <button
        type="submit"
        disabled={!active && blocked !== null}
        title={active ? undefined : (blocked ?? undefined)}
        className={`min-w-24 rounded-sm border px-3 py-1 font-medium disabled:opacity-60 ${
          active ? "border-rule bg-panel hover:bg-sunken" : "border-led bg-led-wash hover:brightness-95"
        }`}
      >
        {link.state === "connecting" ? "Cancel" : active ? "Disconnect" : "Connect"}
      </button>
    </form>
  );
}
