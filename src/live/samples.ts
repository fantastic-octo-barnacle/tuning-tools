// Sample history for the scope and its legend. Frames arrive from the session about 30 times a
// second (layout in studio-core `frame.rs`). Every value in a frame shares the frame's tick
// times, so the store keeps one ring of times and one column per watched value, aligned row for
// row. A value missing from a frame reads NaN for those rows, as does a failed read; the scope
// draws both as gaps.

const MAGIC = 0x31535454; // "TTS1" little-endian
const CAPACITY = 1 << 17; // about 2 minutes at 1 kHz

const times = new Float64Array(CAPACITY);
const columns = new Map<number, Float64Array>();
let head = 0; // next write index
let len = 0;
let latestTime = 0;
let version = 0;

/** Ring index of logical row `k`, oldest first */
const slot = (k: number) => (head - len + k + CAPACITY) % CAPACITY;

/** Stands in for a value with no samples yet */
let empty: Float64Array | null = null;

function column(id: number) {
  let c = columns.get(id);
  if (!c) columns.set(id, (c = new Float64Array(CAPACITY).fill(NaN)));
  return c;
}

/** First logical row whose time is at or after `t` */
function lowerBound(t: number) {
  let lo = 0;
  let hi = len;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (times[slot(mid)] < t) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/** One lane's data for uPlot: shared x, one y array per id, with nulls for gaps */
type Shaped = [number[], ...(number | null)[][]];

export interface Window {
  data: Shaped;
  /** Lowest and highest value of each id over the window; null when it has none */
  ranges: ([number, number] | null)[];
}

export const samples = {
  get version() {
    return version;
  },
  get latestTime() {
    return latestTime;
  },
  latest(id: number): number | undefined {
    const c = columns.get(id);
    return c && len ? c[slot(len - 1)] : undefined;
  },
  forget(id: number) {
    columns.delete(id);
  },
  /** A new session restarts its clock at zero. */
  clear() {
    columns.clear();
    head = 0;
    len = 0;
    latestTime = 0;
    version++;
  },
  ingest(buf: ArrayBuffer) {
    const header = new Uint32Array(buf, 0, 4);
    if (header[0] !== MAGIC) return;
    const n = header[1];
    const count = header[2];
    const start = head;
    // Copy through the ring one row at a time; frames are a few dozen rows
    const frameTimes = new Float64Array(buf, 16, n);
    for (let i = 0; i < n; i++) times[(start + i) % CAPACITY] = frameTimes[i];
    const seen = new Set<number>();
    let at = 16 + 8 * n;
    for (let c = 0; c < count; c++) {
      const id = new Uint32Array(buf, at, 1)[0];
      const values = new Float64Array(buf, at + 8, n);
      at += 8 + 8 * n;
      const col = column(id);
      for (let i = 0; i < n; i++) col[(start + i) % CAPACITY] = values[i];
      seen.add(id);
    }
    // Values not in this frame have no reading for its rows
    for (const [id, col] of columns) {
      if (!seen.has(id)) for (let i = 0; i < n; i++) col[(start + i) % CAPACITY] = NaN;
    }
    head = (start + n) % CAPACITY;
    len = Math.min(CAPACITY, len + n);
    if (n) latestTime = frameTimes[n - 1];
    version++;
  },

  /**
   * The ids' samples between `from` and `to`, shaped for a plot `pixels` wide. With
   * more than two samples per column, each column keeps the lowest and highest value of each
   * id, in the order they happened, so peaks survive and the draw cost stays flat.
   */
  window(ids: number[], from: number, to: number, pixels: number): Window {
    const i0 = lowerBound(from);
    const i1 = lowerBound(to + 1e-9);
    const cs = ids.map((id) => columns.get(id) ?? (empty ??= new Float64Array(CAPACITY).fill(NaN)));
    const m = cs.length;
    const xs: number[] = [];
    const ys = cs.map((): (number | null)[] => []);
    const lo = new Float64Array(m).fill(Infinity);
    const hi = new Float64Array(m).fill(-Infinity);
    if (i1 - i0 <= pixels * 2) {
      for (let k = i0; k < i1; k++) {
        const p = slot(k);
        xs.push(times[p]);
        for (let s = 0; s < m; s++) {
          const v = cs[s][p];
          if (Number.isNaN(v)) ys[s].push(null);
          else {
            ys[s].push(v);
            if (v < lo[s]) lo[s] = v;
            if (v > hi[s]) hi[s] = v;
          }
        }
      }
    } else {
      const width = (to - from) / pixels;
      const bMin = new Float64Array(m);
      const bMax = new Float64Array(m);
      const iMin = new Int32Array(m);
      const iMax = new Int32Array(m);
      let k = i0;
      while (k < i1) {
        const bucket = Math.floor((times[slot(k)] - from) / width);
        const end = from + (bucket + 1) * width;
        bMin.fill(Infinity);
        bMax.fill(-Infinity);
        let j = k;
        for (; j < i1; j++) {
          const p = slot(j);
          if (times[p] >= end) break;
          for (let s = 0; s < m; s++) {
            // NaN compares false, so failed reads drop out of the bucket
            const v = cs[s][p];
            if (v < bMin[s]) {
              bMin[s] = v;
              iMin[s] = j;
            }
            if (v > bMax[s]) {
              bMax[s] = v;
              iMax[s] = j;
            }
          }
        }
        const x0 = from + bucket * width;
        xs.push(x0 + width * 0.25, x0 + width * 0.75);
        for (let s = 0; s < m; s++) {
          if (bMin[s] === Infinity) {
            ys[s].push(null, null);
            continue;
          }
          const minFirst = iMin[s] <= iMax[s];
          ys[s].push(minFirst ? bMin[s] : bMax[s], minFirst ? bMax[s] : bMin[s]);
          if (bMin[s] < lo[s]) lo[s] = bMin[s];
          if (bMax[s] > hi[s]) hi[s] = bMax[s];
        }
        k = j;
      }
    }
    return {
      data: [xs, ...ys],
      ranges: ids.map((_, s) => (lo[s] === Infinity ? null : [lo[s], hi[s]])),
    };
  },
};
