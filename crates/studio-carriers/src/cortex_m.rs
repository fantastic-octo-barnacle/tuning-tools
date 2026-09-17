//! Cortex-M debug registers read through plain memory access.

use crate::{CoreState, MemoryAccess, Result};

/// Debug Halting Control and Status Register (ARMv6-M, ARMv7-M, ARMv8-M)
pub const DHCSR: u64 = 0xE000_EDF0;
const S_HALT: u32 = 1 << 17;
const S_SLEEP: u32 = 1 << 18;
const S_LOCKUP: u32 = 1 << 19;

/// Read the run state from DHCSR. Reading it never halts the core.
pub fn core_state(memory: &mut dyn MemoryAccess) -> Result<CoreState> {
    let mut word = [0u8; 4];
    memory.read(DHCSR, &mut word)?;
    Ok(decode_dhcsr(u32::from_le_bytes(word)))
}

pub fn decode_dhcsr(dhcsr: u32) -> CoreState {
    if dhcsr & S_LOCKUP != 0 {
        CoreState::LockedUp
    } else if dhcsr & S_HALT != 0 {
        CoreState::Halted
    } else if dhcsr & S_SLEEP != 0 {
        CoreState::Sleeping
    } else {
        CoreState::Running
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_run_state() {
        assert_eq!(decode_dhcsr(0x0100_0000), CoreState::Running);
        assert_eq!(decode_dhcsr(0x0003_0003), CoreState::Halted);
        assert_eq!(decode_dhcsr(0x0004_0001), CoreState::Sleeping);
        assert_eq!(decode_dhcsr(0x000A_0001), CoreState::LockedUp);
    }
}
