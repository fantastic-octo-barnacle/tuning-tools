//! Wire format on stdin and stdout.
//!
//! Every message, in both directions, is one frame:
//!
//! ```text
//! u32 length (little-endian, bytes after this field) | u8 kind | payload
//! ```
//!
//! - kind `b'J'`: a UTF-8 JSON object.
//! - kind `b'F'` (server to client only): a sample frame,
//!   `u32 session | u32 reserved | TTS1 bytes` (TTS1 layout in studio-core `frame.rs`).
//!   The 8-byte header keeps the TTS1 bytes 8-byte aligned within the payload.
//!
//! Client to server, JSON: `{"id": 1, "method": "open_elf", "params": {...}}`. Methods
//! and params are the Tauri commands' names and arguments, plus `session` on
//! `session_connect` (the client picks it; the session's events and frames carry it).
//!
//! Server to client, JSON:
//! - `{"type": "ready", "version": "0.1.0", "mock": false, "readOnly": false}`, once, first
//! - `{"type": "response", "id": 1, "ok": true, "result": ...}`, or `"ok": false, "error": "..."`
//! - `{"type": "event", "session": 3, "event": {"type": "status" | "stats" | "log" | "tune" | "catalog", ...}}`
//! - `{"type": "app_event", "event": {"type": "recording" | "stream", ...}}`: recording
//!   progress about once a second and when it ends, and the TCP stream's state, over
//!   every session (`studio_app::AppEvent`)
//!
//! Besides the Tauri commands' methods: `recording_start` takes an optional `dir`,
//! where a recording without a `path` goes (the desktop app uses its data directory).

use std::io::{self, Read};

pub const KIND_JSON: u8 = b'J';
pub const KIND_FRAME: u8 = b'F';
/// Largest message accepted from the client
pub const MAX_MESSAGE: usize = 64 << 20;

/// One whole message, length prefix included.
pub fn encode(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + payload.len());
    out.extend_from_slice(&(payload.len() as u32 + 1).to_le_bytes());
    out.push(kind);
    out.extend_from_slice(payload);
    out
}

/// A sample frame message for `session`.
pub fn encode_frame(session: u32, tts1: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(13 + tts1.len());
    out.extend_from_slice(&(tts1.len() as u32 + 9).to_le_bytes());
    out.push(KIND_FRAME);
    out.extend_from_slice(&session.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(tts1);
    out
}

/// The next message as `(kind, payload)`; `None` at a clean end of input.
pub fn read_message(input: &mut impl Read) -> io::Result<Option<(u8, Vec<u8>)>> {
    let mut len = [0u8; 4];
    match input.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len) as usize;
    if len == 0 || len > MAX_MESSAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("bad message length {len}"),
        ));
    }
    let mut body = vec![0u8; len];
    input.read_exact(&mut body)?;
    let kind = body.remove(0);
    Ok(Some((kind, body)))
}
