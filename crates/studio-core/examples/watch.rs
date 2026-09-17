//! Hardware smoke test without the app: attach, sample statics, print the log.
//!
//! ```text
//! cargo run -p studio-core --example watch -- --chip STM32H723VG --elf <firmware> \
//!     [--probe VID:PID[:SERIAL]] [--rate 200] [--secs 5] [--list] <symbol path>...
//! ```
//! `--list` prints RAM statics with numeric types instead of sampling.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use studio_carriers::probe::{list_probes, ProbeConfig, ProbeLink};
use studio_carriers::Link;
use studio_core::frame;
use studio_core::session::SessionOptions;
use studio_core::{ReadItem, Session, SessionCommand, SessionEvent, SessionSink};
use studio_dwarf::tree::{self, NodeKind};
use studio_dwarf::{ElfParser, NodeRef};

struct Print {
    last: Mutex<Vec<(u32, f64)>>,
    ticks: Mutex<usize>,
}

impl SessionSink for Print {
    fn frame(&self, bytes: Vec<u8>) -> bool {
        let f = frame::decode(&bytes).expect("frame");
        *self.ticks.lock().unwrap() += f.times.len();
        *self.last.lock().unwrap() = f
            .columns
            .iter()
            .map(|(id, v)| (*id, *v.last().unwrap()))
            .collect();
        true
    }

    fn event(&self, event: SessionEvent) {
        match event {
            SessionEvent::Log { lines } => {
                for l in lines {
                    println!(
                        "  log {:>10} {:<5} {}",
                        l.timestamp.unwrap_or_default(),
                        l.level.unwrap_or_default(),
                        l.message
                    );
                }
            }
            other => println!("{other:?}"),
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut chip, mut elf_path, mut probe) = (None, None, None);
    let (mut rate, mut secs, mut list) = (200.0, 5u64, false);
    let mut symbols = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--chip" => chip = args.next(),
            "--elf" => elf_path = args.next(),
            "--probe" => probe = args.next(),
            "--rate" => rate = args.next().unwrap().parse().unwrap(),
            "--secs" => secs = args.next().unwrap().parse().unwrap(),
            "--list" => list = true,
            _ => symbols.push(a),
        }
    }
    let elf_path = elf_path.expect("--elf <firmware>");
    let elf = ElfParser::parse(&elf_path).expect("parse ELF");

    if list {
        for root in tree::roots(&elf)
            .iter()
            .filter(|r| !r.read_only && r.node.readable)
        {
            if matches!(root.node.kind, NodeKind::Scalar | NodeKind::Enum) {
                println!(
                    "{:#010x} {:<8} {}",
                    root.node.address, root.node.type_name, root.node.path
                );
            }
        }
        return;
    }

    println!("probes: {:?}", list_probes());
    let mut items = Vec::new();
    for (i, path) in symbols.iter().enumerate() {
        let n = tree::node(&elf, &NodeRef::root(path.clone())).expect("symbol");
        println!(
            "watch {i}: {} {} @ {:#010x}",
            n.path, n.type_name, n.address
        );
        items.push(ReadItem {
            id: i as u32,
            address: n.address,
            scalar: n.scalar.expect("numeric symbol"),
            bit_offset: None,
            bit_size: None,
        });
    }

    let config = ProbeConfig {
        selector: probe,
        chip: chip.expect("--chip <probe-rs chip name>"),
        speed_khz: None,
        rtt_address: elf.find_symbol("_SEGGER_RTT").map(|s| s.address),
    };
    println!(
        "RTT control block: {:?}",
        config.rtt_address.map(|a| format!("{a:#010x}"))
    );
    let sink = Arc::new(Print {
        last: Mutex::new(Vec::new()),
        ticks: Mutex::new(0),
    });
    let session = Session::spawn(
        move || ProbeLink::open(&config).map(|l| Box::new(l) as Box<dyn Link>),
        SessionOptions {
            rate_hz: rate,
            elf: Some(std::fs::read(&elf_path).unwrap()),
        },
        sink.clone(),
    );
    session.send(SessionCommand::SetWatches(items));
    for _ in 0..secs {
        std::thread::sleep(Duration::from_secs(1));
        println!(
            "ticks {:>6}  values {:?}",
            sink.ticks.lock().unwrap(),
            sink.last.lock().unwrap()
        );
    }
    drop(session);
}
