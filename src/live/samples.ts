// Sample history for the scope and the watch table. Frames arrive from the
// session about 30 times a second (layout in studio-core `frame.rs`); each
// watched value keeps a bounded ring of (time, value) pairs.

const MAGIC = 0x31535454; // "TTS1" little-endian
const CAPACITY = 1 << 17; // about 2 minutes at 1 kHz

class Series {
  t = new Float64Array(CAPACITY);
  v = new Float64Array(CAPACITY);
  head = 0; // next write index
  len = 0;

  push(t: number, v: number) {
    this.t[this.head] = t;
    this.v[this.head] = v;
    this.head = (this.head + 1) % CAPACITY;
    if (this.len < CAPACITY) this.len++;
  }

  latest(): number | undefined {
    return this.len ? this.v[(this.head - 1 + CAPACITY) % CAPACITY] : undefined;
  }

  /** Samples with time >= `from`, oldest first, as plain arrays. */
  since(from: number): { xs: number[]; ys: (number | null)[] } {
    const xs: number[] = [];
    const ys: (number | null)[] = [];
    // Walk back from the newest sample to find the window start
    let count = 0;
    while (count < this.len && this.t[(this.head - 1 - count + 2 * CAPACITY) % CAPACITY] >= from) count++;
    for (let k = count; k > 0; k--) {
      const i = (this.head - k + CAPACITY) % CAPACITY;
      xs.push(this.t[i]);
      const v = this.v[i];
      ys.push(Number.isNaN(v) ? null : v);
    }
    return { xs, ys };
  }
}

const series = new Map<number, Series>();
let latestTime = 0;
let version = 0;

export const samples = {
  get version() {
    return version;
  },
  get latestTime() {
    return latestTime;
  },
  latest(id: number) {
    return series.get(id)?.latest();
  },
  since(id: number, from: number) {
    return series.get(id)?.since(from) ?? { xs: [], ys: [] };
  },
  forget(id: number) {
    series.delete(id);
  },
  /** A new session restarts its clock at zero. */
  clear() {
    series.clear();
    latestTime = 0;
    version++;
  },
  ingest(message: ArrayBuffer | Uint8Array | number[]) {
    const buf = toAlignedBuffer(message);
    const head = new Uint32Array(buf, 0, 4);
    if (head[0] !== MAGIC) return;
    const n = head[1];
    const columns = head[2];
    const times = new Float64Array(buf, 16, n);
    let at = 16 + 8 * n;
    for (let c = 0; c < columns; c++) {
      const id = new Uint32Array(buf, at, 1)[0];
      const values = new Float64Array(buf, at + 8, n);
      at += 8 + 8 * n;
      let s = series.get(id);
      if (!s) series.set(id, (s = new Series()));
      for (let i = 0; i < n; i++) s.push(times[i], values[i]);
    }
    if (n) latestTime = times[n - 1];
    version++;
  },
};

function toAlignedBuffer(message: ArrayBuffer | Uint8Array | number[]): ArrayBuffer {
  if (message instanceof ArrayBuffer) return message;
  if (message instanceof Uint8Array) {
    // Typed-array views need the frame to start at an 8-byte boundary
    return message.buffer.slice(message.byteOffset, message.byteOffset + message.byteLength) as ArrayBuffer;
  }
  return new Uint8Array(message).buffer;
}
