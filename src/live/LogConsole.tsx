import { useLayoutEffect, useMemo, useRef, useState } from "react";
import { LogLine, StreamState } from "./api";

const LEVELS = ["trace", "debug", "info", "warn", "error"];
const SHOWN = 1000;

const levelClass: Record<string, string> = {
  trace: "text-muted",
  debug: "text-muted",
  info: "text-scalar",
  warn: "text-led",
  error: "text-danger",
};

interface Props {
  lines: LogLine[];
  stream: StreamState | null;
  connected: boolean;
  onClear: () => void;
}

export function LogConsole({ lines, stream, connected, onClear }: Props) {
  const [minLevel, setMinLevel] = useState("debug");
  const [filter, setFilter] = useState("");
  const list = useRef<HTMLDivElement>(null);
  const follow = useRef(true);

  const needle = filter.trim().toLowerCase();
  const visible = useMemo(() => {
    const min = LEVELS.indexOf(minLevel);
    const out = lines.filter(
      (l) =>
        (l.level === null || LEVELS.indexOf(l.level) >= min) &&
        (!needle || l.message.toLowerCase().includes(needle) || l.module?.toLowerCase().includes(needle)),
    );
    return out.length > SHOWN ? out.slice(out.length - SHOWN) : out;
  }, [lines, minLevel, needle]);

  useLayoutEffect(() => {
    const el = list.current;
    if (el && follow.current) el.scrollTop = el.scrollHeight;
  }, [visible]);

  let empty = "Connect to see the firmware's defmt log.";
  if (connected) {
    if (!stream || stream.state === "absent") empty = "This firmware has no RTT log (no _SEGGER_RTT symbol or defmt table).";
    else if (stream.state === "searching") empty = "Waiting for the firmware to set up RTT…";
    else empty = needle ? `No log line contains “${filter.trim()}”.` : "No log lines yet.";
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex items-center gap-3 border-b border-rule px-3 py-1.5">
        <h2 className="font-medium">Log</h2>
        <input
          value={filter}
          onChange={(e) => setFilter(e.currentTarget.value)}
          placeholder="Filter"
          spellCheck={false}
          className="w-32 min-w-0 flex-1 rounded-sm border border-rule bg-surface px-2 py-0.5 font-mono text-[12px] placeholder:text-muted"
        />
        <select
          value={minLevel}
          onChange={(e) => setMinLevel(e.currentTarget.value)}
          aria-label="Lowest level shown"
          className="rounded-sm border border-rule bg-surface px-1 py-0.5"
        >
          {LEVELS.map((l) => (
            <option key={l} value={l}>{l} and up</option>
          ))}
        </select>
        <button onClick={onClear} className="rounded-sm px-2 py-0.5 text-muted hover:bg-sunken hover:text-ink">
          Clear
        </button>
      </div>
      <div
        ref={list}
        role="log"
        aria-live="off"
        onScroll={(e) => {
          const el = e.currentTarget;
          follow.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
        }}
        className="min-h-0 flex-1 overflow-auto py-1 font-mono text-[12px] leading-[18px] select-text"
      >
        {visible.length === 0 ? (
          <p className="px-3 py-2 font-sans text-[13px] text-muted">{empty}</p>
        ) : (
          visible.map((l, i) => (
            <div key={i} className="flex gap-2 px-3 hover:bg-sunken/50" title={l.location ?? undefined}>
              <span className="w-16 shrink-0 text-right text-muted tabular-nums">{l.timestamp ?? l.hostTime.toFixed(3)}</span>
              <span className={`w-10 shrink-0 ${levelClass[l.level ?? ""] ?? "text-muted"}`}>{l.level ?? ""}</span>
              <span className="min-w-0 break-words whitespace-pre-wrap">{l.message}</span>
            </div>
          ))
        )}
      </div>
    </div>
  );
}
