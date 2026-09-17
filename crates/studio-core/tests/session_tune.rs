//! Tuning through a session: the mock target holds the fixture firmware's image.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use studio_carriers::mock::MockLink;
use studio_carriers::Link;
use studio_core::catalog::{Catalog, ElfImage, TableLayout};
use studio_core::session::SessionOptions;
use studio_core::tune::CatalogCheck;
use studio_core::{Session, SessionCommand, SessionEvent, SessionSink};
use studio_dwarf::ElfParser;

const ELF: &[u8] = include_bytes!("fixtures/rm_telemetry.elf");

#[derive(Default)]
struct Collect(Mutex<Vec<SessionEvent>>);

impl SessionSink for Collect {
    fn frame(&self, _: Vec<u8>) -> bool {
        true
    }
    fn event(&self, event: SessionEvent) {
        self.0.lock().unwrap().push(event);
    }
}

impl Collect {
    fn last_tune(&self) -> Option<SessionEvent> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|e| matches!(e, SessionEvent::Tune { .. }))
            .cloned()
    }
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn fixture() -> (TableLayout, Catalog, MockLink) {
    let elf = ElfParser::parse_bytes(ELF, "rm_telemetry.elf").unwrap();
    let layout = TableLayout::find(&elf).unwrap().unwrap();
    let image = ElfImage::parse(ELF).unwrap();
    let mock = MockLink::new();
    for (at, data) in image.sections() {
        mock.poke(at, data);
    }
    let mut image = image;
    let catalog = layout.read(&mut image).unwrap();
    (layout, catalog, mock)
}

fn start(mock: &MockLink, layout: TableLayout, catalog: Catalog) -> (Session, Arc<Collect>) {
    let sink = Arc::new(Collect::default());
    let link = mock.clone();
    let session = Session::spawn(
        move || Ok(Box::new(link) as Box<dyn Link>),
        SessionOptions {
            rate_hz: 100.0,
            elf: None,
            rtt_address: None,
        },
        sink.clone(),
    );
    session.send(SessionCommand::SetCatalog(Some(Box::new((
        layout, catalog,
    )))));
    (session, sink)
}

fn request(session: &Session, id: u32, value: f64) -> Result<(), String> {
    let (reply, rx) = mpsc::sync_channel(1);
    assert!(session.send(SessionCommand::Request { id, value, reply }));
    rx.recv_timeout(Duration::from_secs(5)).expect("a reply")
}

#[test]
fn requests_land_in_the_cell_and_show_in_the_values() {
    let (layout, catalog, mock) = fixture();
    let kp = catalog.entries[0].clone();
    let (session, sink) = start(&mock, layout, catalog);

    wait_for("the table check", || {
        matches!(
            sink.last_tune(),
            Some(SessionEvent::Tune {
                check: CatalogCheck::Matches,
                ..
            })
        )
    });
    let Some(SessionEvent::Tune { values, .. }) = sink.last_tune() else {
        unreachable!()
    };
    assert_eq!(values.len(), 6);
    assert_eq!((values[0].id, values[0].requested), (kp.id, Some(40.0)));

    request(&session, kp.id, 55.5).unwrap();
    assert_eq!(mock.peek(kp.requested_address, 4), 55.5f32.to_le_bytes());
    wait_for(
        "the request in the values",
        || matches!(sink.last_tune(), Some(SessionEvent::Tune { values, .. }) if values[0].requested == Some(55.5)),
    );

    let err = request(&session, kp.id, 500.0).unwrap_err();
    assert!(err.contains("0 to 200"), "{err}");
    let err = request(&session, 0xdead_beef, 1.0).unwrap_err();
    assert!(err.contains("no value"), "{err}");
    let err = request(&session, values[2].id, 1.0).unwrap_err();
    assert!(err.contains("read-only"), "{err}");
    assert_eq!(mock.peek(kp.requested_address, 4), 55.5f32.to_le_bytes());
}

#[test]
fn a_target_running_another_build_refuses_writes() {
    let (layout, catalog, mock) = fixture();
    let kp = catalog.entries[0].clone();
    // The target's first entry has another name, as a different build would
    let first_name = catalog.entries[0].name.clone();
    let image = ElfImage::parse(ELF).unwrap();
    let at = image
        .sections()
        .find_map(|(at, data)| {
            data.windows(first_name.len())
                .position(|w| w == first_name.as_bytes())
                .map(|i| at + i as u64)
        })
        .expect("name bytes in the image");
    mock.poke(at, b"X");

    let (session, sink) = start(&mock, layout, catalog);
    wait_for("the table check", || {
        matches!(
            sink.last_tune(),
            Some(SessionEvent::Tune {
                check: CatalogCheck::Differs { .. },
                ..
            })
        )
    });
    assert!(
        matches!(sink.last_tune(), Some(SessionEvent::Tune { values, .. }) if values.is_empty()),
        "no values from another build's cells"
    );
    let err = request(&session, kp.id, 50.0).unwrap_err();
    assert!(err.contains("not running the open ELF"), "{err}");
    assert_eq!(mock.peek(kp.requested_address, 4), 40.0f32.to_le_bytes());
}
