import { useEffect, useMemo, useState } from "react";
import { SymbolNode, Task, shortLocation } from "../elf/api";
import { RowNote, SymbolTree } from "../elf/SymbolTree";
import { Carrier, TaskStatus, taskStates } from "./api";

/** Between reads of every task's state; people read these, they do not plot them */
const POLL_MS = 500;

function taskNote(status: TaskStatus): RowNote {
  const { state } = status;
  if (!state) return { text: "unreadable", tone: "danger", title: status.error ?? undefined };
  if (!state.spawned) return { text: "not running", tone: "muted", title: "Not spawned, or already finished" };
  const at = state.at;
  if (!at) {
    return state.queued ? { text: "ready", tone: "led", title: "Woken, waiting to be polled" } : { text: "waiting", tone: "ink" };
  }
  if (at.label === "Unresumed") return { text: "not started", tone: "led", title: "Spawned, not yet polled" };
  if (at.label === "Returned") return { text: "returned", tone: "muted" };
  if (at.label === "Panicked") return { text: "panicked", tone: "danger" };
  const where = at.location ? shortLocation(at.location) : at.label;
  const full = at.location ? `${at.location.file}:${at.location.line}` : "an unknown line";
  return state.queued
    ? { text: `ready · ${where}`, tone: "led", title: `Woken, waiting to be polled; parked at ${full} (${at.label})` }
    : { text: where, tone: "ink", title: `Waiting at ${full} (${at.label})` };
}

interface Props {
  tasks: Task[];
  connected: boolean;
  carrier: Carrier | null;
  selected: SymbolNode | null;
  onSelect: (node: SymbolNode) => void;
  onWatch: (node: SymbolNode) => void;
  watched: Set<string>;
}

export function TasksPanel({ tasks, connected, carrier, selected, onSelect, onWatch, watched }: Props) {
  const [statuses, setStatuses] = useState<TaskStatus[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const live = connected && carrier === "probe";

  useEffect(() => {
    setStatuses(null);
    setError(null);
    if (!live) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    // One read in flight at a time: the next starts after this one lands
    const poll = () =>
      taskStates()
        .then(
          (s) => {
            if (stopped) return;
            setStatuses(s);
            setError(null);
          },
          (e) => !stopped && setError(String(e)),
        )
        .finally(() => {
          if (!stopped) timer = setTimeout(poll, POLL_MS);
        });
    void poll();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }, [live, tasks]);

  const roots = useMemo(() => tasks.map((t) => t.root), [tasks]);
  const notes = useMemo(() => {
    const out = new Map<string, RowNote>();
    for (const s of statuses ?? []) {
      out.set(s.path, taskNote(s));
      if (s.state?.spawned && s.state.at) out.set(s.state.at.path, { text: "current state", tone: "led" });
    }
    return out;
  }, [statuses]);

  const running = statuses?.filter((s) => s.state?.spawned).length ?? 0;
  const ready = statuses?.filter((s) => s.state?.queued).length ?? 0;
  let summary: React.ReactNode;
  if (carrier === "serial" && connected) summary = "Task states need a debug probe; the USB link cannot read memory.";
  else if (!live) summary = "Connect a debug probe to see where each task is waiting.";
  else if (error) summary = <span className="text-danger">Could not read task states: {error}</span>;
  else if (statuses) summary = `${running} of ${tasks.length} running, ${ready} ready to be polled`;
  else summary = "Reading task states…";

  return (
    <div className="flex h-full min-h-0 flex-col">
      <p className="border-b border-rule px-3 py-1.5 text-[12px] text-muted">{summary}</p>
      <div className="min-h-0 flex-1">
        <SymbolTree
          roots={roots}
          selected={selected}
          onSelect={onSelect}
          onWatch={onWatch}
          watched={watched}
          notes={notes}
          filters={false}
          label="Tasks"
        />
      </div>
    </div>
  );
}
