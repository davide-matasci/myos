# myos

A small, readable x86_64 kernel in Rust. It boots in QEMU and prints
`Hello from myos` to the VGA text buffer. This is a starting point to grow
into a real OS, not a feature dump.

It uses the classic [phil-opp](https://os.phil-opp.com/minimal-rust-kernel/)
BIOS path: **bootloader 0.9** + **bootimage** + a custom target. That still
maps VGA text memory at `0xb8000`, so you can print with a few volatile
stores and no extra crates. bootloader 0.11+ is more capable (UEFI, pixel
framebuffer) but needs a workspace, `bootloader_api`, and a framebuffer
font stack — overkill for a hello kernel.

## Prerequisites

- [rustup](https://rustup.rs/) (the `rust-toolchain.toml` in this repo
  selects nightly and the components below)
- QEMU (`qemu-system-x86_64`)
- The `bootimage` cargo subcommand

Nightly components (installed automatically by rustup from
`rust-toolchain.toml`):

- `rust-src` — rebuild `core` for the custom target
- `llvm-tools-preview` — `bootimage` needs `llvm-objcopy` and friends

```sh
# rustup reads rust-toolchain.toml on the first cargo invocation
# and installs nightly + rust-src + llvm-tools-preview.

cargo install bootimage

# Debian/Ubuntu:  sudo apt install qemu-system-x86
# Fedora:         sudo dnf install qemu-system-x86
# macOS:          brew install qemu
# Arch:           sudo pacman -S qemu-system-x86
```

## Build and run

From the repo root:

```sh
cargo run
```

That compiles the kernel for `x86_64-myos.json`, links it with the BIOS
bootloader, and starts QEMU. You should see a green `Hello from myos` on a
black screen.

To only build the disk image:

```sh
cargo bootimage
```

The image lands at:

```
target/x86_64-myos/debug/bootimage-myos.bin
```

Boot it yourself with:

```sh
qemu-system-x86_64 -drive format=raw,file=target/x86_64-myos/debug/bootimage-myos.bin
```

Release build:

```sh
cargo run --release
```

## Layout

| Path | Role |
|------|------|
| `src/main.rs` | `no_std` entry (`_start`), panic handler, halt loop |
| `src/vga_buffer.rs` | VGA text writer at `0xb8000` |
| `x86_64-myos.json` | Bare-metal target (no red zone, soft-float, rust-lld) |
| `.cargo/config.toml` | default target, `build-std`, `bootimage` runner |
| `rust-toolchain.toml` | nightly + `rust-src` + `llvm-tools-preview` |

## Notes

- The CPU is halted with `hlt` after printing. Close the QEMU window to exit.
- There is no keyboard, paging, heap, or interrupt handling yet.
