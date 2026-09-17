import { useEffect, useState } from "react";
import { samples } from "./samples";
import { formatValue } from "./format";
import { MAX_TRACES, Watch } from "./useWatches";

const REFRESH_MS = 100;

interface Props {
  watches: Watch[];
  onTogglePlot: (id: number) => void;
  onRemove: (id: number) => void;
  onClear: () => void;
}

export function WatchTable({ watches, onTogglePlot, onRemove, onClear }: Props) {
  // Values change far faster than people read; repaint at 10 Hz
  const [, setTick] = useState(0);
  useEffect(() => {
    const timer = setInterval(() => setTick((t) => t + 1), REFRESH_MS);
    return () => clearInterval(timer);
  }, []);
  const plotted = watches.filter((w) => w.plotted).length;

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex items-center gap-3 border-b border-rule px-3 py-1.5">
        <h2 className="font-medium">Watch</h2>
        <span className="text-[12px] text-muted tabular-nums">
          {watches.length} {watches.length === 1 ? "value" : "values"}, {plotted} of {MAX_TRACES} plotted
        </span>
        {watches.length > 0 && (
          <button onClick={onClear} className="ml-auto rounded-sm px-2 py-0.5 text-muted hover:bg-sunken hover:text-ink">
            Remove all
          </button>
        )}
      </div>
      <div className="min-h-0 flex-1 overflow-auto">
        {watches.length === 0 ? (
          <p className="p-4 text-muted">
            Select a number or struct in the symbol tree and press W, or use its Watch button.
          </p>
        ) : (
          <table className="w-full table-fixed border-collapse text-[12px]">
            <colgroup>
              <col className="w-8" />
              <col />
              <col className="w-[22%]" />
              <col className="w-[28%]" />
              <col className="w-8" />
            </colgroup>
            <tbody>
              {watches.map((w) => {
                const value = samples.latest(w.id);
                const failed = value !== undefined && Number.isNaN(value);
                const canPlot = w.plotted || plotted < MAX_TRACES;
                return (
                  <tr key={w.id} className="border-b border-rule/60 hover:bg-sunken/50">
                    <td className="py-1 pl-3">
                      <button
                        onClick={() => onTogglePlot(w.id)}
                        disabled={!canPlot}
                        aria-pressed={w.plotted}
                        title={w.plotted ? "Stop plotting" : canPlot ? "Plot" : `At most ${MAX_TRACES} traces`}
                        className="flex h-4 w-4 items-center justify-center rounded-sm border border-rule disabled:opacity-40"
                        style={w.plotted && w.trace !== null ? { background: `var(--trace-${w.trace + 1})`, borderColor: "transparent" } : undefined}
                      >
                        <span className="sr-only">{w.plotted ? "Plotted" : "Not plotted"}</span>
                      </button>
                    </td>
                    <td className="truncate py-1 pl-1 font-mono" title={w.path}>
                      {w.path}
                    </td>
                    <td className="truncate py-1 pl-2 font-mono text-[11px] text-muted" title={w.typeName}>
                      {w.typeName}
                    </td>
                    <td
                      className={`truncate py-1 pr-2 text-right font-mono tabular-nums ${w.error || failed ? "text-danger" : ""}`}
                      title={w.error ?? undefined}
                    >
                      {w.error ? "cannot sample" : formatValue(value, w.scalar)}
                    </td>
                    <td className="py-1 pr-2 text-center">
                      <button
                        onClick={() => onRemove(w.id)}
                        aria-label={`Stop watching ${w.path}`}
                        className="rounded-sm px-1 text-muted hover:bg-sunken hover:text-ink"
                      >
                        ×
                      </button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
