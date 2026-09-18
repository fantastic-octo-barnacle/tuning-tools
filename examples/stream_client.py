#!/usr/bin/env python3
"""Read Tuning Studio's live data stream (docs/stream.md) and print a rolling mean per watch.

Start the stream from the status bar ("Stream off" -> Start), then:

    python3 examples/stream_client.py                 # 127.0.0.1:7878
    python3 examples/stream_client.py --port 7879 --window 500
    python3 examples/stream_client.py --csv run.csv   # also write every tick

Standard library only. The CSV has a `time` column and one per watch; when the watch set
changes, a new header row starts the new columns.
"""

import argparse
import csv
import json
import math
import socket
import sys
import time
from collections import deque


class Rolling:
    """Mean of the last `size` finite values"""

    def __init__(self, size):
        self.values = deque(maxlen=size)
        self.total = 0.0

    def push(self, v):
        if len(self.values) == self.values.maxlen:
            self.total -= self.values[0]
        self.values.append(v)
        self.total += v

    def mean(self):
        return self.total / len(self.values) if self.values else math.nan


def lines(sock):
    """The stream's messages, one JSON object per line"""
    buffered = b""
    while True:
        chunk = sock.recv(65536)
        if not chunk:
            return
        buffered += chunk
        *whole, buffered = buffered.split(b"\n")
        for line in whole:
            if line:
                yield json.loads(line)


def describe(watches):
    for w in watches:
        unit = f" [{w['unit']}]" if w.get("unit") else ""
        print(f"  #{w['id']:<4} {w['name']}{unit}  {w.get('path') or ''}  {w.get('type') or ''}")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=7878)
    parser.add_argument("--window", type=int, default=1000, help="samples in each rolling mean (default 1000)")
    parser.add_argument("--every", type=float, default=1.0, help="seconds between printouts (default 1)")
    parser.add_argument("--csv", metavar="PATH", help="also write every tick to this CSV file")
    parser.add_argument("--seconds", type=float, help="stop after this long")
    args = parser.parse_args()

    try:
        sock = socket.create_connection((args.host, args.port), timeout=5)
    except OSError as e:
        sys.exit(f"could not connect to {args.host}:{args.port}: {e} (is the stream started?)")
    sock.settimeout(None)

    out = open(args.csv, "w", newline="") if args.csv else None
    writer = csv.writer(out) if out else None
    names = []
    means = {}
    ticks = batches = dropped = 0
    started = time.monotonic()
    next_print = started + args.every

    def set_watches(watches):
        nonlocal names, means
        names = [w["name"] for w in watches]
        means = {n: Rolling(args.window) for n in names}
        if writer and watches:
            writer.writerow(["time"] + [f"{w['name']} [{w['unit']}]" if w.get("unit") else w["name"] for w in watches])

    try:
        for m in lines(sock):
            kind = m.get("type")
            if kind == "hello":
                s = m["session"]
                print(f"hello: protocol {m['version']}, {s.get('elf') or 'no ELF'}, "
                      f"{s.get('state')}, {s.get('rateHz')} Hz, app {s.get('appVersion')}")
                print(f"watching {len(m['watches'])}:")
                describe(m["watches"])
                set_watches(m["watches"])
            elif kind == "watches":
                print(f"watches changed, now {len(m['watches'])}:")
                describe(m["watches"])
                set_watches(m["watches"])
            elif kind == "samples":
                batches += 1
                ticks += len(m["t"])
                values = m["values"]
                for name, column in values.items():
                    mean = means.get(name)
                    if mean is None:
                        continue
                    for v in column:
                        if v is not None:
                            mean.push(v)
                if writer:
                    columns = [values.get(n, [None] * len(m["t"])) for n in names]
                    for i, t in enumerate(m["t"]):
                        writer.writerow([t] + ["" if c[i] is None else c[i] for c in columns])
            elif kind == "dropped":
                dropped += m.get("batches", 0)
                print(f"dropped {m.get('batches', 0)} batches (this client fell behind)")
            elif kind == "status":
                print(f"status: {m['state']}" + (f" ({m['message']})" if m.get("message") else ""))
            elif kind == "log":
                for line in m["lines"]:
                    print(f"log: {line.get('level') or '-'} {line['message']}")
            # `tune` and future message types are ignored here

            now = time.monotonic()
            if now >= next_print and names:
                next_print = now + args.every
                shown = "  ".join(f"{n}={means[n].mean():.4g}" for n in names[:8])
                more = f"  (+{len(names) - 8} more)" if len(names) > 8 else ""
                print(f"[{now - started:6.1f} s] {ticks} ticks in {batches} batches  mean: {shown}{more}")
            if args.seconds and now - started >= args.seconds:
                break
    except KeyboardInterrupt:
        pass
    finally:
        sock.close()
        if out:
            out.close()
    print(f"done: {ticks} ticks in {batches} batches, {dropped} batches dropped"
          + (f", CSV in {args.csv}" if args.csv else ""))


if __name__ == "__main__":
    main()
