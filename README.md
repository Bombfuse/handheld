# Handheld OS

A gaming handheld platform built on [VexiiRiscv](https://github.com/SpinalHDL/VexiiRiscv) and the [k23 microkernel](https://github.com/JonasKruckenberg/k23). Games are WebAssembly modules running on a custom SDK with a 320x240 indexed-color display, 4-channel audio, and gamepad input.

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs/) (stable + nightly toolchains)
- [QEMU](https://www.qemu.org/) RISC-V system emulator
- [binaryen](https://github.com/WebAssembly/binaryen) (`wasm-opt`)
- [just](https://just.systems/) command runner

```sh
# Arch
sudo pacman -S qemu-system-riscv qemu-system-riscv-firmware binaryen just

# Ubuntu/Debian
sudo apt install qemu-system-misc binaryen
cargo install just

# macOS
brew install qemu binaryen just
```

Then add the WASM target:

```sh
rustup target add wasm32-unknown-unknown
```

### Run

```sh
git clone <repo-url> handheld && cd handheld
make run
```

This builds the games, packs them into cartridges, compiles the k23 microkernel with Cranelift JIT, and boots it on QEMU. A GTK window opens showing the game launcher.

**Click the QEMU window** to capture keyboard input, then:

- **Arrow keys** -- navigate menu / move in game
- **Enter** -- start / select
- **W/A/S/D** -- alternative movement
- **Q** -- return to launcher

The first run takes a few minutes (JIT-compiling the Rust kernel + Cranelift for RISC-V). Subsequent runs are fast.

### Desktop Simulator (Alternative)

For faster iteration during game development, use the desktop host-runner:

```sh
make run-desktop
```

This uses [wasmtime](https://wasmtime.dev/) + [minifb](https://crates.io/crates/minifb) instead of QEMU — same SDK, same games, instant startup with audio.

## Writing a Game

Create a new crate in `games/`:

```toml
# games/mygame/Cargo.toml
[package]
name = "mygame"
edition = "2024"
[lib]
crate-type = ["cdylib"]
[dependencies]
handheld-sdk = { path = "../../sdk" }
```

```rust
// games/mygame/src/lib.rs
#![no_std]
use handheld_sdk::*;

#[unsafe(no_mangle)]
pub extern "C" fn init() {
    set_palette(0, 0, 0, 0);       // black
    set_palette(1, 255, 255, 255);  // white
}

#[unsafe(no_mangle)]
pub extern "C" fn update() {
    if button_pressed(Button::A) { /* ... */ }
}

#[unsafe(no_mangle)]
pub extern "C" fn draw() {
    clear(0);
    text("Hello!", 120, 110, 1);
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}
```

Add it to the workspace `Cargo.toml`, then build and pack:

```sh
cargo build -p mygame --target wasm32-unknown-unknown --release
cargo run -p cart-packer -- --name "My Game" \
  --wasm target/wasm32-unknown-unknown/release/mygame.wasm \
  -o carts/mygame.cart
make run
```

## Project Structure

```
handheld/
  sdk/                  Game SDK (Rust, compiles to WASM)
  libs/cart/            Cart file format (no_std parser + std writer)
  sim/host-runner/      Desktop runner (wasmtime + minifb + cpal)
  tools/cart-packer/    CLI to package games as .cart files
  games/
    hello/              Demo: text + shapes
    snake/              Playable snake game
    launcher/           System menu (game selector)
  deps/k23/             Forked k23 microkernel with:
    kernel/src/
      ramfb.rs          RAM framebuffer driver (QEMU display)
      virtio_input.rs   VirtIO keyboard driver (QEMU input)
      shell.rs          Game loader + launcher state machine
```

## SDK Reference

| Category | Functions |
|----------|-----------|
| Graphics | `clear`, `pixel`, `line`, `rect`, `rect_fill`, `circle`, `circle_fill`, `text`, `set_palette` |
| Sprites  | `sprite`, `sprite_region` |
| Tilemap  | `tilemap_set`, `tilemap_get`, `tilemap_scroll`, `tilemap_clear`, `tilemap_draw` |
| Audio    | `tone`, `tone_slide` |
| Input    | `button`, `button_pressed`, `button_released` |
| System   | `trace`, `random`, `frame_count`, `quit` |

**Display specs:** 320x240, 256-color indexed, RGB565 palette.

**Buttons:** `Up`, `Down`, `Left`, `Right`, `A`, `B`, `X`, `Y`, `Start`, `Select`, `L`, `R`.

## Architecture

```
┌─────────────────────────────────────────┐
│              QEMU riscv64 virt          │
│  ┌───────────────────────────────────┐  │
│  │         k23 microkernel           │  │
│  │  ┌─────────────┐ ┌─────────────┐ │  │
│  │  │ Cranelift   │ │ ramfb       │ │  │
│  │  │ JIT compile │ │ display     │ │  │
│  │  └──────┬──────┘ └──────┬──────┘ │  │
│  │         │               │        │  │
│  │    ┌────┴────┐    ┌─────┴─────┐  │  │
│  │    │  WASM   │    │  virtio   │  │  │
│  │    │  Game   │    │  keyboard │  │  │
│  │    └─────────┘    └───────────┘  │  │
│  └───────────────────────────────────┘  │
│         8-core RV64GC, 256MB RAM        │
└─────────────────────────────────────────┘
```

Games are compiled to WASM, packed as `.cart` files, embedded in the k23 kernel image, JIT-compiled to native RISC-V machine code by Cranelift at boot, and rendered to a RAM framebuffer displayed via QEMU's GTK window.

## License

MIT
