import { useEffect, useState } from "react";

/** What the scope's plots know that its readouts show; written on every frame and cursor move */
export interface Readout {
  /** Time under the cursor, while hovering */
  cursorT: number | null;
  /** Each plotted value under the cursor; null in a gap */
  cursor: Map<number, number | null>;
  /** Lowest and highest value of each plotted value over the window */
  ranges: Map<number, [number, number] | null>;
  listeners: Set<() => void>;
}

export function newReadout(): Readout {
  return { cursorT: null, cursor: new Map(), ranges: new Map(), listeners: new Set() };
}

export function notify(readout: Readout) {
  readout.listeners.forEach((f) => f());
}

/** Re-render on readout changes, at most once a frame, and every `ms` for new samples */
export function useReadout(readout: Readout, ms: number) {
  const [, setTick] = useState(0);
  useEffect(() => {
    let raf = 0;
    const bump = () => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => setTick((t) => t + 1));
    };
    readout.listeners.add(bump);
    const timer = setInterval(bump, ms);
    return () => {
      readout.listeners.delete(bump);
      clearInterval(timer);
      cancelAnimationFrame(raf);
    };
  }, [readout, ms]);
}
