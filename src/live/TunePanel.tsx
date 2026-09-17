import { useMemo, useState } from "react";
import { Catalog, CatalogEntry } from "../elf/api";
import { formatValue } from "./format";
import { discardValues, requestValue, saveValues } from "./api";
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
  return `${entry.min} to ${entry.max}${unit}`;
}

export function TunePanel({ catalog, catalogError, tune, connected, fromTarget, watched, onWatch }: Props) {
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

  async function run(action: "reset" | "save") {
    setBusy(action);
    setNotice(null);
    try {
      if (action === "reset") {
        await discardValues();
      } else {
        await saveValues();
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
      <div className="flex items-center gap-2 border-b border-rule px-3 py-1.5 text-[12px]">
        <span className="min-w-0 flex-1">
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
        </span>
        <button
          type="button"
          disabled={!canWrite || busy !== null}
          onClick={() => void run("reset")}
          title="Request every value's built-in default"
          className="shrink-0 rounded-sm border border-rule bg-panel px-2 py-0.5 hover:bg-sunken disabled:opacity-40"
        >
          Reset all
        </button>
        <button
          type="button"
          disabled={!canWrite || busy !== null}
          onClick={() => void run("save")}
          title="Store the requested values on the robot so they survive a power cycle"
          className="shrink-0 rounded-sm border border-led bg-led-wash px-2 py-0.5 hover:brightness-95 disabled:opacity-40"
        >
          {busy === "save" ? "Saving…" : "Save to robot"}
        </button>
      </div>
      {notice && (
        <p
          role={notice.error ? "alert" : "status"}
          className={`border-b border-rule px-3 py-1 text-[12px] ${notice.error ? "text-danger" : "text-muted"}`}
        >
          {notice.text}
        </p>
      )}
      <div className="min-h-0 flex-1 overflow-auto">
        {grouped.map((group) => (
          <section key={group.name}>
            <h3 className="sticky top-0 border-b border-rule bg-panel px-3 py-1 font-mono text-[12px] text-muted">
              {group.name || "values"}
            </h3>
            <ul>
              {group.entries.map((entry) => (
                <Row
                  key={entry.id}
                  entry={entry}
                  value={tune?.values.get(entry.id) ?? null}
                  canWrite={canWrite}
                  watched={watched.has(entry.name)}
                  onWatch={() => onWatch(entry)}
                />
              ))}
            </ul>
          </section>
        ))}
      </div>
    </div>
  );
}

interface RowProps {
  entry: CatalogEntry;
  value: { requested: number | null; applied: number | null } | null;
  canWrite: boolean;
  watched: boolean;
  onWatch: () => void;
}

function Row({ entry, value, canWrite, watched, onWatch }: RowProps) {
  const live = entry.access !== "readOnly";
  const [draft, setDraft] = useState<string | null>(null);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requested = value?.requested ?? null;
  const applied = value?.applied ?? null;
  const settling = live && requested !== null && applied !== null && requested !== applied;

  async function send(text: string) {
    const number = Number(text.trim());
    if (text.trim() === "" || !Number.isFinite(number)) {
      setError("Enter a number.");
      return;
    }
    setSending(true);
    try {
      await requestValue(entry.id, number);
      setDraft(null);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setSending(false);
    }
  }

  return (
    <li className="border-b border-rule/60 px-3 py-1.5">
      <div className="flex items-baseline gap-2">
        <span className="min-w-0 flex-1 truncate font-mono text-[13px]" title={entry.name}>
          {leaf(entry.name)}
        </span>
        <span
          className={`font-mono tabular-nums ${settling ? "text-led" : ""}`}
          title={settling ? "Moving toward the request at the firmware's step limit" : "Value the firmware runs with"}
        >
          {applied === null ? (value ? "read failed" : "") : formatValue(applied, entry.kind)}
        </span>
        <button
          onClick={onWatch}
          disabled={watched}
          title={watched ? "On the watch list" : "Watch and plot"}
          className="rounded-sm px-1.5 text-[12px] text-muted hover:bg-sunken hover:text-ink disabled:opacity-40 disabled:hover:bg-transparent"
        >
          {watched ? "Watched" : "Watch"}
        </button>
      </div>
      <div className="mt-1 flex items-center gap-2 text-[12px] text-muted">
        {live ? (
          <form
            className="flex items-center gap-1.5"
            onSubmit={(e) => {
              e.preventDefault();
              if (draft !== null) void send(draft);
            }}
          >
            <label className="sr-only" htmlFor={`req-${entry.id}`}>
              Request for {entry.name}
            </label>
            <input
              id={`req-${entry.id}`}
              inputMode="decimal"
              disabled={!canWrite || sending}
              value={draft ?? (requested === null ? "" : formatValue(requested, entry.kind))}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  setDraft(null);
                  setError(null);
                }
              }}
              onBlur={() => {
                if (!sending) setDraft(null);
              }}
              className={`w-24 rounded-sm border bg-surface px-1.5 py-0.5 text-right font-mono tabular-nums text-ink disabled:opacity-60 ${
                draft !== null ? "border-led" : "border-rule"
              }`}
            />
            <button
              type="button"
              disabled={!canWrite || sending || requested === entry.default}
              onClick={() => void send(String(entry.default))}
              title={`Request the built-in value, ${formatValue(entry.default, entry.kind)}`}
              className="rounded-sm px-1.5 hover:bg-sunken hover:text-ink disabled:opacity-40 disabled:hover:bg-transparent"
            >
              Default
            </button>
          </form>
        ) : (
          <span>read-only</span>
        )}
        {entry.access === "safeOnly" && (
          <span title="The firmware refuses changes while the robot is armed">while disarmed</span>
        )}
        <span className="ml-auto truncate" title={entry.maxStep ? `At most ${entry.maxStep} per control tick` : undefined}>
          {range(entry)}
        </span>
      </div>
      {error && <p className="mt-1 text-[12px] text-danger">{error}</p>}
    </li>
  );
}
