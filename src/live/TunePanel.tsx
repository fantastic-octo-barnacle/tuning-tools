import { useEffect, useMemo, useState } from "react";
import { Catalog, CatalogEntry } from "../elf/api";
import { host } from "../host";
import { ghostButton, primaryButton } from "../ui";
import { formatValue } from "./format";
import { Tune } from "./useSession";

interface Props {
  catalog: Catalog | null;
  catalogError: string | null;
  tune: Tune | null;
  connected: boolean;
  /** The catalog came from the firmware itself, so there is no build to check */
  fromTarget: boolean;
  /** Names of the values on the watch list */
  watched: Set<string>;
  onWatch: (entry: CatalogEntry) => void;
  onSave: () => Promise<void>;
}

interface Group {
  name: string;
  entries: CatalogEntry[];
}

function groups(catalog: Catalog): Group[] {
  const out: Group[] = [];
  for (const entry of catalog.entries) {
    const cut = entry.name.lastIndexOf(".");
    const name = cut < 0 ? "" : entry.name.slice(0, cut);
    const last = out[out.length - 1];
    if (last && last.name === name) last.entries.push(entry);
    else out.push({ name, entries: [entry] });
  }
  return out;
}

function leaf(name: string) {
  return name.slice(name.lastIndexOf(".") + 1);
}

function range(entry: CatalogEntry) {
  const unit = entry.unit ? ` ${entry.unit}` : "";
  if (entry.min === null || entry.max === null) return entry.unit;
  return `${entry.min} – ${entry.max}${unit}`;
}

export function TunePanel({ catalog, catalogError, tune, connected, fromTarget, watched, onWatch, onSave }: Props) {
  const grouped = useMemo(() => (catalog ? groups(catalog) : []), [catalog]);
  const [busy, setBusy] = useState<"reset" | "save" | null>(null);
  const [notice, setNotice] = useState<{ text: string; error: boolean } | null>(null);

  if (!catalog) {
    return (
      <p className="p-4 leading-relaxed text-muted">
        {catalogError ??
          "This firmware declares no tuning table. Add an rm_telemetry::Table to list its gains and state here."}
      </p>
    );
  }

  const check = tune?.check ?? null;
  const canWrite = connected && check?.state === "matches";
  const unsaved = catalog.entries.filter((e) => {
    const requested = tune?.values.get(e.id)?.requested ?? null;
    return requested !== null && requested !== tune?.saved.get(e.id);
  }).length;

  async function run(action: "reset" | "save") {
    setBusy(action);
    setNotice(null);
    try {
      if (action === "reset") {
        await host.discardValues();
        setNotice({ text: "Defaults requested. Save to keep them.", error: false });
      } else {
        await onSave();
        setNotice({ text: "Saved. The robot starts with these values after a power cycle.", error: false });
      }
    } catch (e) {
      setNotice({ text: String(e), error: true });
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <p className="border-b border-grid px-3 py-1.5 text-[12px]">
        {!connected ? (
          <span className="text-muted">Connect to read and change these values on the target.</span>
        ) : check === null || check.state === "checking" ? (
          <span className="text-muted">Checking that the target runs this build…</span>
        ) : check.state === "differs" ? (
          <span role="alert" className="text-danger">
            {check.message}
          </span>
        ) : (
          <span className="text-muted">
            {fromTarget ? "Values listed by the firmware." : "Target runs this build."} Press Enter to send a value.
          </span>
        )}
      </p>
      <div className="min-h-0 flex-1 overflow-auto pb-3">
        {grouped.map((group) => (
          <section key={group.name}>
            <h3 className="px-3 pt-2 pb-0.5 text-[10.5px] tracking-[.05em] text-muted uppercase">{group.name || "values"}</h3>
            <ul>
              {group.entries.map((entry) => (
                <Row
                  key={entry.id}
                  entry={entry}
                  value={tune?.values.get(entry.id) ?? null}
                  saved={tune?.saved.get(entry.id) ?? null}
                  canWrite={canWrite}
                  watched={watched.has(entry.name)}
                  onWatch={() => onWatch(entry)}
                />
              ))}
            </ul>
          </section>
        ))}
      </div>
      <div className="flex flex-wrap items-center gap-1.5 border-t border-rule bg-panel px-3 py-2 text-[12px]">
        <button
          disabled={!canWrite || busy !== null || unsaved === 0}
          onClick={() => void run("save")}
          title="Store the requested values on the robot so they survive a power cycle"
          className={`${primaryButton} text-[12px]`}
        >
          {busy === "save" ? "Saving…" : unsaved ? `Save ${unsaved} to robot flash` : "Save to robot flash"}
        </button>
        <button
          disabled={!canWrite || busy !== null}
          onClick={() => void run("reset")}
          title="Request every value's built-in default"
          className={ghostButton}
        >
          Reset to defaults
        </button>
        {notice && (
          <span role={notice.error ? "alert" : "status"} className={`basis-full ${notice.error ? "text-danger" : "text-muted"}`}>
            {notice.text}
          </span>
        )}
      </div>
    </div>
  );
}

interface RowProps {
  entry: CatalogEntry;
  value: { requested: number | null; applied: number | null } | null;
  /** The requested value at the last save, as far as this session knows */
  saved: number | null;
  canWrite: boolean;
  watched: boolean;
  onWatch: () => void;
}

const pill = "inline-block rounded-full border px-[7px] text-[10.5px] leading-4";

function Row({ entry, value, saved, canWrite, watched, onWatch }: RowProps) {
  const live = entry.access !== "readOnly";
  const [draft, setDraft] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** The last number sent, to explain a value the firmware changed; `seen` once a read followed it */
  const [sent, setSent] = useState<{ value: number; seen: boolean } | null>(null);
  useEffect(() => setSent((s) => (s && !s.seen ? { ...s, seen: true } : s)), [value]);
  const requested = value?.requested ?? null;
  const applied = value?.applied ?? null;
  const settling = live && requested !== null && applied !== null && requested !== applied;
  const id = `req-${entry.id}`;

  async function send(text: string) {
    const number = Number(text.trim());
    if (text.trim() === "" || !Number.isFinite(number)) {
      setError("Enter a number.");
      return;
    }
    setSending(true);
    try {
      await host.requestValue(entry.id, number);
      setDraft(null);
      setError(null);
      setSent({ value: number, seen: false });
    } catch (e) {
      setError(String(e));
    } finally {
      setSending(false);
    }
  }

  let note: string | null = null;
  // An f32 cell holds the nearest float to what was sent
  const same = (a: number, b: number) => (entry.kind === "f32" ? Math.fround(a) === Math.fround(b) : a === b);
  if (sent?.seen && requested !== null && !same(requested, sent.value)) {
    const outside = (entry.min !== null && sent.value < entry.min) || (entry.max !== null && sent.value > entry.max);
    note = outside
      ? `The firmware runs ${formatValue(requested, entry.kind)}: ${sent.value} is outside ${range(entry)}.`
      : `The firmware runs ${formatValue(requested, entry.kind)} instead of ${sent.value}.`;
  }

  const state =
    requested === null ? null : requested !== saved ? (
      <span className={`${pill} border-accent bg-accent-wash text-ink`} title="Save to keep it after a power cycle">
        Not saved
      </span>
    ) : requested === entry.default ? (
      <span className={`${pill} border-rule text-muted`}>Default</span>
    ) : (
      <span className={`${pill} border-good text-good`}>Saved</span>
    );

  return (
    <li className="grid grid-cols-[minmax(0,1fr)_92px] items-center gap-x-2 gap-y-0.5 border-b border-grid px-3 py-1.5">
      <label htmlFor={live ? id : undefined} className="truncate font-mono text-[12px]" title={entry.name}>
        {leaf(entry.name)}
      </label>
      {live ? (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            if (draft !== null) void send(draft);
          }}
        >
          <input
            id={id}
            inputMode="decimal"
            spellCheck={false}
            disabled={!canWrite || sending}
            value={draft ?? (requested === null ? "" : formatValue(requested, entry.kind))}
            onChange={(e) => {
              setDraft(e.target.value);
              setSent(null);
            }}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                setDraft(null);
                setError(null);
              }
            }}
            onBlur={() => {
              if (!sending) setDraft(null);
            }}
            className={`w-full rounded-sm border bg-plot px-1.5 py-0.5 text-right font-mono text-[12px] tabular-nums disabled:opacity-60 ${
              draft !== null ? "border-accent" : "border-rule"
            }`}
          />
        </form>
      ) : (
        <span className="text-right font-mono text-[12px] tabular-nums" title="Read-only: the firmware reports it">
          {applied === null ? (value ? "read failed" : "") : formatValue(applied, entry.kind)}
        </span>
      )}
      <div className="col-span-2 flex min-w-0 items-center gap-2 text-[11px] text-faint">
        <span className="truncate" title={entry.maxStep ? `At most ${entry.maxStep} per control tick` : undefined}>
          {range(entry)}
        </span>
        {live && (
          <button
            type="button"
            disabled={!canWrite || sending || requested === entry.default}
            onClick={() => void send(String(entry.default))}
            title="Request the built-in value"
            className="shrink-0 rounded-sm px-0.5 enabled:hover:bg-sunken enabled:hover:text-ink"
          >
            default {formatValue(entry.default, entry.kind)}
          </button>
        )}
        {settling && (
          <span className="shrink-0 text-warn" title="Moving toward the request at the firmware's step limit">
            running {formatValue(applied, entry.kind)}
          </span>
        )}
        {entry.access === "safeOnly" && (
          <span className="shrink-0" title="The firmware refuses changes while the robot is armed">
            while disarmed
          </span>
        )}
        {!live && <span className="shrink-0">read-only</span>}
        <span className="ml-auto flex shrink-0 items-center gap-1.5">
          <button
            onClick={onWatch}
            disabled={watched}
            title={watched ? "On the watch list" : "Watch and plot"}
            className="rounded-sm px-1 enabled:hover:bg-sunken enabled:hover:text-ink disabled:text-accent"
          >
            {watched ? "Watched" : "Watch"}
          </button>
          {live && state}
        </span>
      </div>
      {error && <p className="col-span-2 text-[11px] text-danger">{error}</p>}
      {!error && note && <p className="col-span-2 text-[11px] text-warn">{note}</p>}
    </li>
  );
}
