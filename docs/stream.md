# Live data stream

Tuning Studio can serve the running session over TCP so your own scripts can use every sample
as it is read. The stream carries the same ticks that recordings get, at the full sampling
rate, whether or not the scope keeps up.

Start it from the status bar (**Stream off** → **Start**), or with the `stream_start` command
(`{"port": 7878, "bindAll": false}`). By default it listens on `127.0.0.1:7878`. It listens on
every interface only when you tick **Allow other machines** (`bindAll`). There is no
authentication, so anyone who can reach the port can read the values.

The stream is read-only. Anything a client sends is read and thrown away.

An example client is `examples/stream_client.py`. It uses only the Python standard library,
prints the watches, keeps a rolling mean per watch, and can write a CSV:

```sh
python3 examples/stream_client.py --csv run.csv
```

## Framing

Each message is one JSON object on one line, UTF-8, ending in `\n`. Every message has a
`type`. Clients should ignore types and fields they do not know; new ones may be added without
changing `version`.

Numbers are JSON numbers. A value that could not be read (a failed read or NaN) is `null`.

## Lifetime

- A client can connect at any time, even when no session is running. It first gets a `hello`.
- Clients stay connected when the session disconnects and connects again. They see `status`
  messages as the link changes, and samples resume on the next connect.
- Stopping the stream, or closing the app, closes every connection.

## Messages

### `hello`

This is always the first message on a connection.

```json
{"type":"hello","version":1,
 "session":{"state":"connected","connected":true,
            "elf":"firmware","elfPath":"/path/to/firmware","buildId":"4f1c…","elfMatch":{"state":"matches"},
            "chip":"STM32H723VGTx","carrier":"probe","port":null,"rateHz":1000.0,
            "appVersion":"0.1.0","tunables":[{"id":0,"name":"gimbal.yaw.kp"}],
            "startedAt":"2026-09-18T17:44:04.770966+08:00","startedAtUnixNs":1789724644770966000},
 "watches":[{"id":1,"name":"GIMBAL.yaw.angle","path":"gimbal::GIMBAL.yaw.angle",
             "type":"f32","unit":"rad"}]}
```

- `version` is the protocol version. This document describes `1`.
- `session` describes the latest session. Its fields are `null` before the first connect.
  - `state` is `connecting`, `connected`, `disconnected` or `failed`.
  - `carrier` is `probe` or `serial`.
  - `elfMatch` says whether the firmware's tuning table matches the ELF.
  - `startedAt` and `startedAtUnixNs` give the wall-clock time of session time zero.
- `watches` is the current watch set. Each watch has:
  - `name`: unique within the set, and the key used in `samples`;
  - `path`: the symbol path, or the tuning value's name;
  - `type`, `unit`: either may be `null`;
  - `id`: the app's id for the watch.

### `watches`

This is sent when the watch set changes, or when a watch's name or unit changes. It comes before
the first `samples` message that uses the new set.

```json
{"type":"watches","watches":[{"id":1,"name":"GIMBAL.yaw.angle","path":"gimbal::GIMBAL.yaw.angle","type":"f32","unit":"rad"}]}
```

### `samples`

One message is sent for each batch the session flushes, about 30 per second. Each batch holds
every tick since the last one.

```json
{"type":"samples","seq":812,"t":[12.001,12.002,12.003],
 "values":{"GIMBAL.yaw.angle":[0.52,0.521,null],"CHASSIS.mode":[2,2,2]}}
```

- `t` holds session time in seconds. It starts at 0 when the session connects, so it starts
  over on every connect. For wall-clock time, add `t` to the session's `startedAtUnixNs`.
- `values` has one array per watch, the same length as `t` and keyed by watch name.
- `seq` counts batches across the whole stream, not per client. A gap in `seq` means batches
  were lost, and a `dropped` message follows.

### `dropped`

This client fell behind, and messages were discarded instead of being queued.

```json
{"type":"dropped","batches":4,"messages":0}
```

`batches` counts `samples` messages that were lost. `messages` counts other messages that were
lost. Each client has its own bounded queue (256 messages). A client that stays full for more
than 150 batches in a row, about 5 seconds, is disconnected. A slow client never slows down
other clients or sampling.

### `status`

The link changed state. `session` is the same object as in `hello`.

```json
{"type":"status","state":"disconnected","message":null,"session":{…}}
```

### `log`

These are defmt log lines from the firmware.

```json
{"type":"log","lines":[{"hostTime":3.21,"timestamp":"3.208","level":"info","message":"armed",
                         "location":"src/main.rs:42","module":"app"}]}
```

### `tune`

Tuning values that changed:

```json
{"type":"tune","kind":"values","check":{"state":"matches"},
 "values":[{"id":0,"name":"gimbal.yaw.kp","requested":12.0,"applied":12.0}]}
```

Requests made from the app (`action` is `set`, `save` or `discard`):

```json
{"type":"tune","kind":"request","action":"set","id":0,"name":"gimbal.yaw.kp","value":12.5,"ok":true,"error":null}
```

## Possible extensions

Every message is an object with a `type`, so commands from clients could be added later as
`{"type":"…","id":…}` requests with matching replies, without changing any existing message.
Version 1 has no commands.
