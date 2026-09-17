# Test Fixtures

This directory contains test ELF binaries for testing the DWARF parser and ELF loading functionality.

## Fixtures

### test_arm.elf
A minimal ARM Cortex-M4 binary containing:
- `global_counter`: volatile uint32_t at fixed address
- `sensor_data`: volatile float at fixed address
- Simple main loop

**Source:** `test_source.c`

### test_struct.elf
An ARM binary with struct types:
- `SensorData` struct with x, y, value fields
- `sensor_struct`: volatile SensorData instance

**Source:** `test_struct_source.c`

### test_pointer.elf
An ARM binary with pointer types:
- `data_ptr`: uint32_t* pointer
- Pointer dereferencing scenarios

**Source:** `test_pointer_source.c`

## Building Fixtures

To rebuild the fixtures, you need the ARM GCC toolchain:

```bash
# Install ARM GCC (Ubuntu/Debian)
sudo apt-get install gcc-arm-none-eabi

# Install ARM GCC (macOS)
brew install --cask gcc-arm-embedded

# Build
cd tests/fixtures
make all
```

## Linker Script

The fixtures use a simple linker script (`link.ld`) that places:
- `.text` at 0x08000000 (Flash)
- `.data` at 0x20000000 (RAM)
- `.bss` at 0x20001000 (RAM)

This matches typical STM32 memory layouts for testing.

## Rust fixtures

### rust_v0.elf, rust_legacy.elf
The same firmware-shaped program (`rust_embedded/`) built for
`thumbv7em-none-eabihf` with the rm-embedded-rs release profile (thin LTO,
`opt-level = "s"`, 8 codegen units, full debug info). Statics are mangled and
live in nested modules: atomics, a `[u16; 8]`, a `u64`, and a
`Shared<Gimbal<4>>` holding a fieldless enum, `Option<f32>`, a data-carrying
enum, a niche-encoded `Option<NonZeroU32>`, an array and a tuple.

`rust_v0.elf` uses the default v0 mangling (`_R...`); `rust_legacy.elf` uses
legacy mangling (`_ZN...17h<hash>E`), which stable rustc only allows behind
`-Z unstable-options`.

```bash
cd tests/fixtures/rust_embedded
cargo build --release
cp target/thumbv7em-none-eabihf/release/rust_fixture ../rust_v0.elf
RUSTC_BOOTSTRAP=1 RUSTFLAGS="-C link-arg=-Tlink.x -Z unstable-options -C symbol-mangling-version=legacy" \
  cargo build --release --target-dir target-legacy
cp target-legacy/thumbv7em-none-eabihf/release/rust_fixture ../rust_legacy.elf
```

Built with rustc 1.98.1. Type names such as `Atomic<u32>` follow the core
library of that toolchain; rebuilding with another toolchain may change them.
