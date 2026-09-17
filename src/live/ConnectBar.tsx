import { useEffect, useId, useState } from "react";
import * as api from "./api";
import { Link } from "./useSession";

const RATES = [10, 50, 100, 200, 500, 1000];
/** SWD clock; faster makes large reads quicker but needs short, clean wiring */
const SPEEDS = [1000, 4000, 10000];
const SETTINGS_KEY = "connection";

interface Settings {
  probe: string;
  chip: string;
  rateHz: number;
  speedKhz: number;
}

function loadSettings(): Settings {
  const fallback = { probe: "", chip: "", rateHz: 100, speedKhz: 4000 };
  try {
    return { ...fallback, ...JSON.parse(localStorage.getItem(SETTINGS_KEY) ?? "{}") };
  } catch {
    return fallback;
  }
}

interface Props {
  link: Link;
  canConnect: boolean;
  onConnect: (request: api.ConnectRequest) => void;
  onDisconnect: () => void;
}

export function ConnectBar({ link, canConnect, onConnect, onDisconnect }: Props) {
  const [settings, setSettings] = useState(loadSettings);
  const [probes, setProbes] = useState<api.ProbeInfo[] | null>(null);
  const [chips, setChips] = useState<string[]>([]);
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
    else onConnect({ probe: settings.probe || null, chip: settings.chip.trim(), speedKhz: settings.speedKhz, rateHz: settings.rateHz });
  };

  const selectedMissing = settings.probe && probes && !probes.some((p) => p.selector === settings.probe);

  return (
    <form onSubmit={submit} className="ml-auto flex flex-wrap items-center gap-2">
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
        disabled={!active && (!canConnect || !settings.chip.trim())}
        title={!canConnect ? "Open the firmware ELF first" : !settings.chip.trim() ? "Enter the target chip" : undefined}
        className={`min-w-24 rounded-sm border px-3 py-1 font-medium disabled:opacity-60 ${
          active ? "border-rule bg-panel hover:bg-sunken" : "border-led bg-led-wash hover:brightness-95"
        }`}
      >
        {link.state === "connecting" ? "Cancel" : active ? "Disconnect" : "Connect"}
      </button>
    </form>
  );
}
