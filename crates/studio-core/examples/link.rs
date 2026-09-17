//! Hardware smoke test for the framed link without the app.
//!
//! ```text
//! cargo run -p studio-core --example link -- [--port <path>] [--rate 100] [--secs 3] \
//!     [--tune <name>=<value>] [--discard] <value name to plot>...
//! ```
//! Without `--port` the first port reporting the rm-telemetry product is used.

use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use studio_carriers::serial::{list_ports, SerialStream};
use studio_carriers::ByteStream;
use studio_core::link::{spawn_link, LinkOptions};
use studio_core::{frame, SessionCommand, SessionEvent, SessionSink};

/// Samples seen, the latest value per column, and the latest time
type Ticks = (usize, Vec<(u32, f64)>, f64);

#[derive(Default)]
struct Print {
    catalog: Mutex<Option<studio_core::catalog::Catalog>>,
    ticks: Mutex<Ticks>,
}

impl SessionSink for Print {
    fn frame(&self, bytes: Vec<u8>) -> bool {
        let f = frame::decode(&bytes).expect("frame");
        let mut t = self.ticks.lock().unwrap();
        t.0 += f.times.len();
        t.1 = f
            .columns
            .iter()
            .map(|(id, v)| (*id, *v.last().unwrap()))
            .collect();
        t.2 = *f.times.last().unwrap();
        true
    }

    fn event(&self, event: SessionEvent) {
        match event {
            SessionEvent::Catalog { catalog } => {
                println!("catalog: {} values", catalog.entries.len());
                *self.catalog.lock().unwrap() = Some(catalog);
            }
            SessionEvent::Tune { .. } => {}
            SessionEvent::Stats {
                stats, last_error, ..
            } => println!(
                "stats: {:.1} Hz of {:.1}, skipped {}, bad frames {}, error {last_error:?}",
                stats.achieved_hz, stats.target_hz, stats.skipped_ticks, stats.failed_regions
            ),
            other => println!("{other:?}"),
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut port, mut rate, mut secs, mut tune, mut discard) = (None, 100.0, 3, None, false);
    let mut names = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--port" => port = args.next(),
            "--rate" => rate = args.next().unwrap().parse().unwrap(),
            "--secs" => secs = args.next().unwrap().parse().unwrap(),
            "--tune" => tune = args.next(),
            "--discard" => discard = true,
            _ => names.push(a),
        }
    }
    let port = port.unwrap_or_else(|| {
        let ports = list_ports();
        println!("ports: {ports:?}");
        ports
            .into_iter()
            .find(|p| p.telemetry)
            .expect("no rm-telemetry port")
            .path
    });
    let sink = Arc::new(Print::default());
    let open = port.clone();
    let session = spawn_link(
        move || SerialStream::open(&open, 115_200).map(|s| Box::new(s) as Box<dyn ByteStream>),
        LinkOptions { rate_hz: rate },
        sink.clone(),
    );
    std::thread::sleep(Duration::from_millis(500));
    let catalog = sink.catalog.lock().unwrap().clone().expect("a catalog");
    let id = |name: &str| {
        catalog
            .entries
            .iter()
            .find(|e| e.name == name)
            .expect(name)
            .id
    };
    let watches = names
        .iter()
        .enumerate()
        .map(|(i, n)| (i as u32, id(n)))
        .collect();
    session.send(SessionCommand::SetCellWatches(watches));

    let ask = |command: fn(mpsc::SyncSender<Result<(), String>>) -> SessionCommand| {
        let (reply, rx) = mpsc::sync_channel(1);
        session.send(command(reply));
        rx.recv().unwrap()
    };
    if let Some(arg) = &tune {
        let (name, value) = arg.split_once('=').unwrap();
        let (reply, rx) = mpsc::sync_channel(1);
        session.send(SessionCommand::Request {
            id: id(name),
            value: value.parse().unwrap(),
            reply,
        });
        println!("request {name} = {value}: {:?}", rx.recv().unwrap());
    }
    if discard {
        println!(
            "discard: {:?}",
            ask(|reply| SessionCommand::Discard { reply })
        );
    }
    for _ in 0..secs {
        std::thread::sleep(Duration::from_secs(1));
        let t = sink.ticks.lock().unwrap();
        println!("samples {:>6} at t={:.3}s latest {:?}", t.0, t.2, t.1);
    }
    drop(session);
}
