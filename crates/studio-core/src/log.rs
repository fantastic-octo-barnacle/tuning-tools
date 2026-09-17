//! defmt log decoding. The decoder borrows its table, so it lives on its own
//! thread that owns both; the session hands it raw RTT bytes.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use defmt_decoder::{DecodeError, Table};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    /// Host seconds since the session started, when the bytes arrived
    pub host_time: f64,
    /// Firmware timestamp as defmt formats it, e.g. `12.345`
    pub timestamp: Option<String>,
    pub level: Option<String>,
    pub message: String,
    /// `file:line`
    pub location: Option<String>,
    pub module: Option<String>,
}

pub struct LogDecoder {
    tx: Option<Sender<(f64, Vec<u8>)>>,
    thread: Option<JoinHandle<()>>,
}

impl LogDecoder {
    /// Parse the defmt table from `elf`. `Ok(None)` when the firmware has no defmt.
    /// `sink` receives decoded lines in arrival order.
    pub fn spawn(
        elf: Vec<u8>,
        sink: impl FnMut(Vec<LogLine>) + Send + 'static,
    ) -> Result<Option<Self>, String> {
        // Parse before spawning so a bad table is reported to the caller
        let Some(table) = Table::parse(&elf).map_err(|e| format!("defmt table: {e}"))? else {
            return Ok(None);
        };
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("defmt-decoder".into())
            .spawn(move || run(table, elf, rx, sink))
            .map_err(|e| e.to_string())?;
        Ok(Some(Self {
            tx: Some(tx),
            thread: Some(thread),
        }))
    }

    pub fn push(&self, host_time: f64, bytes: Vec<u8>) {
        if let Some(tx) = &self.tx {
            let _ = tx.send((host_time, bytes));
        }
    }
}

impl Drop for LogDecoder {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(
    table: Table,
    elf: Vec<u8>,
    rx: Receiver<(f64, Vec<u8>)>,
    mut sink: impl FnMut(Vec<LogLine>),
) {
    let locations = table.get_locations(&elf).ok().filter(|l| !l.is_empty());
    drop(elf);
    let recover = table.encoding().can_recover();
    let mut decoder = table.new_stream_decoder();
    let mut broken = false;
    while let Ok((host_time, bytes)) = rx.recv() {
        if broken {
            continue;
        }
        decoder.received(&bytes);
        let mut lines = Vec::new();
        loop {
            match decoder.decode() {
                Ok(frame) => {
                    let location = locations.as_ref().and_then(|l| l.get(&frame.index()));
                    lines.push(LogLine {
                        host_time,
                        timestamp: frame.display_timestamp().map(|t| t.to_string()),
                        level: frame.level().map(|l| l.as_str().to_string()),
                        message: frame.display_message().to_string(),
                        location: location.map(|l| format!("{}:{}", l.file.display(), l.line)),
                        module: location.map(|l| l.module.clone()),
                    });
                }
                Err(DecodeError::UnexpectedEof) => break,
                Err(DecodeError::Malformed) if recover => {
                    lines.push(note(host_time, "skipped a malformed defmt frame"));
                }
                Err(DecodeError::Malformed) => {
                    lines.push(note(
                        host_time,
                        "defmt stream is malformed and this encoding cannot resync; reconnect to resume the log",
                    ));
                    broken = true;
                    break;
                }
            }
        }
        if !lines.is_empty() {
            sink(lines);
        }
    }
}

fn note(host_time: f64, message: &str) -> LogLine {
    LogLine {
        host_time,
        timestamp: None,
        level: Some("warn".into()),
        message: format!("[tuning-tools] {message}"),
        location: None,
        module: None,
    }
}
