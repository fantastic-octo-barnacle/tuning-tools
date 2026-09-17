//! Raw SWD read latency through a held MEM-AP handle, by size and clock speed:
//! `cargo run --release -p studio-carriers --example probe_bench -- STM32H723VG`
use std::time::Instant;

use probe_rs::architecture::arm::dp::DpAddress;
use probe_rs::architecture::arm::{ApV2Address, FullyQualifiedApAddress};
use probe_rs::probe::list::Lister;
use probe_rs::Permissions;
use probe_rs_target::{ApAddress, CoreAccessOptions};

fn memory_ap(core: &probe_rs::config::Core) -> FullyQualifiedApAddress {
    let CoreAccessOptions::Arm(options) = &core.core_access_options else {
        panic!("not an Arm core");
    };
    let dp = options
        .targetsel
        .map_or(DpAddress::Default, DpAddress::Multidrop);
    match &options.ap {
        ApAddress::V1(ap) => FullyQualifiedApAddress::v1_with_dp(dp, *ap),
        ApAddress::V2(ap) => FullyQualifiedApAddress::v2_with_dp(dp, ApV2Address::new(*ap)),
    }
}

fn main() {
    let chip = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "STM32H723VG".into());
    let address = 0x2400_4670u64;
    let n = 200;
    for khz in [4000u32, 10000] {
        let probes = Lister::new().list_all();
        let mut probe = probes[0].open().expect("open probe");
        let _ = probe.set_speed(khz);
        let actual = probe.speed_khz();
        let mut session = probe.attach(chip.as_str(), Permissions::default()).expect("attach");
        let ap = memory_ap(&session.target().cores[0]);
        let iface = session.get_arm_interface().unwrap();
        let mut mem = iface.memory_interface(&ap).unwrap();
        let time = |label: &str, f: &mut dyn FnMut()| {
            let start = Instant::now();
            for _ in 0..n {
                f();
            }
            println!("speed {actual:>5} kHz  {label:<22} {:>6.0} us", start.elapsed().as_secs_f64() * 1e6 / n as f64);
        };
        for words in [1usize, 4, 16, 64] {
            let mut bytes = vec![0u8; words * 4];
            time(&format!("read {} B", words * 4), &mut || mem.read(address, &mut bytes).unwrap());
            let mut w = vec![0u32; words];
            time(&format!("read_32 x{words}"), &mut || mem.read_32(address, &mut w).unwrap());
            time(&format!("read_8 {} B", words * 4), &mut || mem.read_8(address, &mut bytes).unwrap());
        }
    }
}
