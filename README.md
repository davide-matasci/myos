# myos

A small, readable x86_64 kernel in Rust. It boots in QEMU and prints
`Hello from myos` to the bootloader framebuffer (and to serial). This is a
starting point to grow into a real OS, not a feature dump.

It uses **[bootloader 0.11+](https://crates.io/crates/bootloader)**
(`bootloader` 0.11.17 / `bootloader_api` 0.11.17): a `kernel` crate plus a
host builder that produces BIOS and UEFI disk images with
`DiskImageBuilder`. VGA text at `0xb8000` is gone — UEFI (and the 0.11 BIOS
path) give you a pixel framebuffer instead.

## Prerequisites

- [rustup](https://rustup.rs/) (the `rust-toolchain.toml` in this repo
  selects nightly and the components below)
- QEMU (`qemu-system-x86_64`)

Nightly components / targets (installed automatically by rustup from
`rust-toolchain.toml`):

- `llvm-tools-preview` — the bootloader crate needs `llvm-objcopy` to build
  its BIOS stages
- `rust-src`
- `x86_64-unknown-none` — kernel target (tier 2, no custom JSON)
- `x86_64-unknown-uefi` — used while building the UEFI bootloader

```sh
# rustup reads rust-toolchain.toml on the first cargo invocation
# and installs nightly + components + targets.

# Debian/Ubuntu:  sudo apt install qemu-system-x86
# Fedora:         sudo dnf install qemu-system-x86
# macOS:          brew install qemu
# Arch:           sudo pacman -S qemu-system-x86
```

There is no `bootimage` runner anymore.

## Build and run

From the repo root:

```sh
cargo run
```

That compiles the kernel for `x86_64-unknown-none`, wraps it in a BIOS disk
image, and starts QEMU. You should see a green `Hello from myos` on a black
screen. Close the QEMU window to exit.

```sh
cargo run -- uefi
```

Same thing over UEFI (OVMF firmware is fetched on first run into
`target/ovmf`).

To only build the images:

```sh
cargo build
```

Copies also land at:

```
target/bios.img
target/uefi.img
```

Boot a built image yourself with:

```sh
qemu-system-x86_64 -drive format=raw,file=target/bios.img -serial stdio
```

Release build:

```sh
cargo run --release
```

Headless check (what CI runs):

```sh
cargo run -- --ci
```

That boots the BIOS image with `-display none`, requires the hello string
on serial, and expects QEMU to exit via `isa-debug-exit`.

## Layout

| Path | Role |
|------|------|
| `src/main.rs` | Host launcher: starts QEMU |
| `build.rs` | `DiskImageBuilder` → BIOS + UEFI images |
| `kernel/src/main.rs` | `no_std` entry (`entry_point!`), serial, halt |
| `kernel/src/framebuffer.rs` | Pixel writer for the bootloader framebuffer |
| `kernel/src/font.rs` | Tiny 8x8 bitmap font |
| `kernel/src/serial.rs` | COM1 UART for QEMU `-serial stdio` |
| `.cargo/config.toml` | `bindeps` (artifact dependencies) |
| `rust-toolchain.toml` | nightly + `llvm-tools-preview` + rust-src + targets |

## Notes

- The CPU is halted with `hlt` after printing (QEMU stays open unless you
  pass `--ci`, which attaches `isa-debug-exit` so QEMU exits).
- There is no keyboard, paging, heap, or interrupt handling yet.
