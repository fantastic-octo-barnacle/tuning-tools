//! Drive studio-server over stdio against the mock target: open an ELF, watch
//! values, receive sample frames, disconnect and shut down.

use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../studio-dwarf/tests/fixtures/test_arm.elf"
);
const TTS1: u32 = u32::from_le_bytes(*b"TTS1");

enum Message {
    Json(Value),
    Frame { session: u32, tts1: Vec<u8> },
}

struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<Message>,
    next_id: u64,
    /// Messages read while waiting for something else
    backlog: Vec<Message>,
}

fn read_all(mut stdout: ChildStdout, tx: mpsc::Sender<Message>) {
    loop {
        let mut len = [0u8; 4];
        if stdout.read_exact(&mut len).is_err() {
            return;
        }
        let mut body = vec![0u8; u32::from_le_bytes(len) as usize];
        stdout.read_exact(&mut body).expect("whole message");
        let message = match body[0] {
            b'J' => Message::Json(serde_json::from_slice(&body[1..]).expect("JSON message")),
            b'F' => Message::Frame {
                session: u32::from_le_bytes(body[1..5].try_into().unwrap()),
                tts1: body[9..].to_vec(),
            },
            k => panic!("unknown message kind {k:#04x}"),
        };
        if tx.send(message).is_err() {
            return;
        }
    }
}

impl Client {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_studio-server"))
            .arg("--mock")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn studio-server");
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || read_all(stdout, tx));
        Self {
            stdin: child.stdin.take(),
            child,
            rx,
            next_id: 1,
            backlog: Vec::new(),
        }
    }

    fn next(&mut self, deadline: Instant) -> Message {
        let left = deadline.saturating_duration_since(Instant::now());
        self.rx
            .recv_timeout(left)
            .expect("message before the deadline")
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let body =
            serde_json::to_vec(&json!({ "id": id, "method": method, "params": params })).unwrap();
        let stdin = self.stdin.as_mut().unwrap();
        stdin
            .write_all(&(body.len() as u32 + 1).to_le_bytes())
            .unwrap();
        stdin.write_all(b"J").unwrap();
        stdin.write_all(&body).unwrap();
        stdin.flush().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match self.next(deadline) {
                Message::Json(v) if v["type"] == "response" && v["id"] == json!(id) => {
                    return if v["ok"] == json!(true) {
                        Ok(v["result"].clone())
                    } else {
                        Err(v["error"].as_str().unwrap_or_default().to_string())
                    };
                }
                other => self.backlog.push(other),
            }
        }
    }

    /// Wait for a message matching `want`, looking at the backlog first.
    fn wait(&mut self, what: &str, mut want: impl FnMut(&Message) -> bool) -> Message {
        if let Some(at) = self.backlog.iter().position(&mut want) {
            return self.backlog.remove(at);
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let m = self.next(deadline);
            if want(&m) {
                return m;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
        }
    }
}

fn is_status(m: &Message, session: u32, state: &str) -> bool {
    matches!(m, Message::Json(v) if v["type"] == "event"
        && v["session"] == json!(session)
        && v["event"]["type"] == "status"
        && v["event"]["state"] == state)
}

#[test]
fn mock_session_over_stdio() {
    let mut client = Client::spawn();
    let ready = client.wait(
        "ready",
        |m| matches!(m, Message::Json(v) if v["type"] == "ready"),
    );
    let Message::Json(ready) = ready else {
        unreachable!()
    };
    assert_eq!(ready["mock"], json!(true));

    // A bad call answers with an error and leaves the server running
    let err = client.call("no_such_method", json!({})).unwrap_err();
    assert!(err.contains("bad request"), "{err}");
    assert!(client
        .call("session_discard", Value::Null)
        .unwrap_err()
        .contains("connect"));

    let opened = client
        .call("open_elf", json!({ "path": FIXTURE }))
        .expect("open_elf");
    let roots = opened["roots"].as_array().unwrap();
    let find = |name: &str| {
        roots
            .iter()
            .find(|r| r["path"] == name)
            .unwrap_or_else(|| panic!("{name} among the roots"))["ref"]
            .clone()
    };
    let counter = find("global_counter");
    let sensor = find("sensor_data");

    let probes = client.call("list_probes", Value::Null).unwrap();
    assert_eq!(probes[0]["selector"], "mock");

    let results = client
        .call(
            "session_set_watches",
            json!({ "watches": [
                { "id": 1, "node": counter, "cell": null },
                { "id": 2, "node": sensor, "cell": null },
            ]}),
        )
        .unwrap();
    assert_eq!(
        results,
        json!([{ "id": 1, "error": null }, { "id": 2, "error": null }])
    );

    let request = json!({
        "carrier": "probe", "probe": null, "chip": "STM32F407VG",
        "speedKhz": null, "port": null, "rateHz": 200.0,
    });
    client
        .call(
            "session_connect",
            json!({ "request": request, "session": 7 }),
        )
        .expect("connect");
    client.wait("connected", |m| is_status(m, 7, "connected"));

    let mut ticks = 0;
    let mut frames = 0;
    while frames < 5 {
        let Message::Frame { session, tts1 } =
            client.wait("a frame", |m| matches!(m, Message::Frame { .. }))
        else {
            unreachable!()
        };
        assert_eq!(session, 7);
        let word = |at: usize| u32::from_le_bytes(tts1[at..at + 4].try_into().unwrap());
        assert_eq!(word(0), TTS1);
        let (n, columns) = (word(4) as usize, word(8) as usize);
        assert_eq!(columns, 2);
        assert_eq!(tts1.len(), 16 + 8 * n + columns * (8 + 8 * n));
        let first_column = 16 + 8 * n;
        assert_eq!(word(first_column), 1);
        assert_eq!(word(first_column + 8 + 8 * n), 2);
        ticks += n;
        frames += 1;
    }
    assert!(ticks >= 5, "{ticks} ticks in 5 frames");

    client.wait(
        "stats",
        |m| matches!(m, Message::Json(v) if v["type"] == "event" && v["event"]["type"] == "stats"),
    );
    let read = client
        .call("session_read_values", json!({ "nodes": [counter] }))
        .unwrap();
    assert_eq!(read[0]["error"], Value::Null);

    client
        .call("session_set_rate", json!({ "hz": 50.0 }))
        .unwrap();
    client.call("session_disconnect", Value::Null).unwrap();
    client.wait("disconnected", |m| is_status(m, 7, "disconnected"));
    assert!(client
        .call("session_read_values", json!({ "nodes": [counter] }))
        .is_err());

    // Closing stdin shuts the server down
    drop(client.stdin.take());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = client.child.try_wait().unwrap() {
            assert!(status.success(), "{status}");
            break;
        }
        assert!(Instant::now() < deadline, "server did not exit");
        std::thread::sleep(Duration::from_millis(20));
    }
}
