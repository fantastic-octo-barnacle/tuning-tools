import { useEffect, useRef, useState } from "react";
import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";
import { samples } from "./samples";
import { Watch } from "./useWatches";

const WINDOWS = [2, 5, 10, 30, 60];

function cssVar(name: string) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

/** Keep at most two points (min and max) per horizontal pixel column. */
function decimate(xs: number[], ys: (number | null)[], from: number, to: number, columns: number) {
  if (xs.length <= columns * 4) return [xs, ys] as const;
  const outX: number[] = [];
  const outY: (number | null)[] = [];
  const width = (to - from) / columns;
  let i = 0;
  while (i < xs.length) {
    const bucketEnd = from + (Math.floor((xs[i] - from) / width) + 1) * width;
    let lo = i;
    let hi = i;
    let j = i;
    let gap = false;
    for (; j < xs.length && xs[j] < bucketEnd; j++) {
      const y = ys[j];
      if (y === null) gap = true;
      else {
        if (ys[lo] === null || y < (ys[lo] as number)) lo = j;
        if (ys[hi] === null || y > (ys[hi] as number)) hi = j;
      }
    }
    for (const k of lo <= hi ? [lo, hi] : [hi, lo]) {
      outX.push(xs[k]);
      outY.push(ys[k]);
    }
    if (gap) {
      outX.push(xs[j - 1]);
      outY.push(null);
    }
    i = j;
  }
  return [outX, outY] as const;
}

interface Props {
  watches: Watch[];
  connected: boolean;
}

export function Scope({ watches, connected }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const plot = useRef<uPlot | null>(null);
  const [windowSec, setWindowSec] = useState(10);
  const [paused, setPaused] = useState(false);
  const [theme, setTheme] = useState(0);
  const view = useRef({ windowSec, paused, xMin: 0, xMax: 10 });
  view.current.windowSec = windowSec;
  view.current.paused = paused;

  const traces = watches.filter((w) => w.plotted && w.trace !== null);
  const traceKey = traces.map((w) => `${w.id}:${w.trace}`).join(",");

  useEffect(() => {
    const media = matchMedia("(prefers-color-scheme: dark)");
    const bump = () => setTheme((t) => t + 1);
    media.addEventListener("change", bump);
    return () => media.removeEventListener("change", bump);
  }, []);

  // Rebuild the chart when the plotted set or theme changes
  useEffect(() => {
    const el = host.current;
    // Mode 2 needs at least one data series; the empty state covers this case
    if (!el || traces.length === 0) return;
    const axis = {
      stroke: cssVar("--muted"),
      grid: { stroke: cssVar("--rule"), width: 1 },
      ticks: { stroke: cssVar("--rule"), width: 1 },
      font: `11px ${cssVar("--font-mono") || "monospace"}`,
    };
    const opts: uPlot.Options = {
      mode: 2,
      width: el.clientWidth,
      height: el.clientHeight,
      legend: { show: false },
      cursor: { drag: { x: false, y: false }, points: { size: 6 } },
      scales: {
        x: { time: false, auto: false, range: () => [view.current.xMin, view.current.xMax] },
        y: {
          auto: true,
          range: (_u, min, max) => {
            if (min === max) return [min - 1, max + 1];
            const pad = (max - min) * 0.08;
            return [min - pad, max + pad];
          },
        },
      },
      axes: [
        { ...axis, values: (_u, ticks) => ticks.map((t) => `${t.toFixed(t % 1 ? 1 : 0)} s`) },
        { ...axis, size: 64 },
      ],
      series: [
        {},
        ...traces.map((w) => ({
          label: w.path,
          stroke: cssVar(`--trace-${(w.trace ?? 0) + 1}`),
          width: 1.5,
          spanGaps: false,
          points: { show: false },
        })),
      ],
    };
    const empty: uPlot.AlignedData = [null as unknown as number[], ...traces.map(() => [[], []] as unknown as number[])];
    const u = new uPlot(opts, empty, el);
    plot.current = u;
    const resize = new ResizeObserver(() => u.setSize({ width: el.clientWidth, height: el.clientHeight }));
    resize.observe(el);
    return () => {
      resize.disconnect();
      u.destroy();
      plot.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [traceKey, theme]);

  // Redraw on animation frames when new samples arrived
  useEffect(() => {
    let raf = 0;
    let drawn = -1;
    let drawnWindow = -1;
    const ids = traces.map((w) => w.id);
    const tick = () => {
      raf = requestAnimationFrame(tick);
      const u = plot.current;
      const v = view.current;
      if (!u || v.paused) return;
      if (samples.version === drawn && v.windowSec === drawnWindow) return;
      drawn = samples.version;
      drawnWindow = v.windowSec;
      const to = Math.max(samples.latestTime, v.windowSec);
      const from = to - v.windowSec;
      v.xMin = from;
      v.xMax = to;
      const columns = Math.max(u.bbox.width / devicePixelRatio, 50);
      const data = ids.map((id) => {
        const { xs, ys } = samples.since(id, from);
        return decimate(xs, ys, from, to, columns) as unknown as number[];
      });
      u.setData([null as unknown as number[], ...data] as uPlot.AlignedData);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [traceKey, theme]);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex items-center gap-3 border-b border-rule px-3 py-1.5">
        <h2 className="font-medium">Scope</h2>
        <ul className="flex min-w-0 flex-1 gap-3 overflow-hidden text-[12px]">
          {traces.map((w) => (
            <li key={w.id} className="flex min-w-0 items-center gap-1.5" title={w.path}>
              <span className="h-0.5 w-3 shrink-0" style={{ background: `var(--trace-${(w.trace ?? 0) + 1})` }} />
              <span className="truncate font-mono text-[11px]">{w.path.split("::").pop()}</span>
            </li>
          ))}
        </ul>
        <label className="flex items-center gap-1.5 text-muted">
          Window
          <select
            value={windowSec}
            onChange={(e) => setWindowSec(Number(e.currentTarget.value))}
            className="rounded-sm border border-rule bg-surface px-1 py-0.5 text-ink"
          >
            {WINDOWS.map((s) => (
              <option key={s} value={s}>{s} s</option>
            ))}
          </select>
        </label>
        <button
          onClick={() => setPaused((p) => !p)}
          aria-pressed={paused}
          className={`rounded-sm border border-rule px-2 py-0.5 ${paused ? "bg-led-wash" : "bg-panel hover:bg-sunken"}`}
        >
          {paused ? "Resume" : "Pause"}
        </button>
      </div>
      <div className="relative min-h-0 flex-1">
        <div ref={host} className="absolute inset-0" />
        {traces.length === 0 && (
          <p className="pointer-events-none absolute inset-0 flex items-center justify-center p-6 text-center text-muted">
            {connected
              ? "Watch a number from the symbol tree to plot it here."
              : "Connect to the target, then watch numbers from the symbol tree to plot them."}
          </p>
        )}
      </div>
    </div>
  );
}
