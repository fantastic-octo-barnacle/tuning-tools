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
