# myos

A small, readable kernel in Rust. It boots in QEMU and prints
`Hello from myos` on serial (and, on x86_64, to the bootloader framebuffer).
This is a starting point to grow into a real OS, not a feature dump.

x86_64 uses **[bootloader 0.11+](https://crates.io/crates/bootloader)**
(`bootloader` 0.11.17 / `bootloader_api` 0.11.17): a `kernel` crate plus a
host builder that produces BIOS and UEFI disk images with
`DiskImageBuilder`.

AArch64 is a second boot path. bootloader 0.11 is x86_64-only, so the ARM
kernel is loaded directly by QEMU's `virt` machine (`-kernel` of the ELF).

Nightly is **pinned** (`nightly-2026-07-26`) because floating latest nightly
fails to link `bootloader-x86_64-uefi` (the `uefi` crate vs rust-lld). That
pin is the day before 0.11.17 was published. Do not unpin it.

## Prerequisites

- [rustup](https://rustup.rs/) (the `rust-toolchain.toml` in this repo
  selects the pinned nightly and the components below)
- QEMU (`qemu-system-x86_64` and, for AArch64, `qemu-system-aarch64`)

Nightly components / targets (installed automatically by rustup from
`rust-toolchain.toml`):

- `llvm-tools-preview` — the bootloader crate needs `llvm-objcopy` to build
  its BIOS stages
- `rust-src`
- `x86_64-unknown-none` — x86_64 kernel target
- `x86_64-unknown-uefi` — used while building the UEFI bootloader
- `aarch64-unknown-none-softfloat` — AArch64 kernel target (softfloat so we
  do not touch NEON/FP before the CPU is set up)

```sh
# rustup reads rust-toolchain.toml on the first cargo invocation
# and installs the pinned nightly + components + targets.

# Debian/Ubuntu:  sudo apt install qemu-system-x86 qemu-system-arm
# Fedora:         sudo dnf install qemu-system-x86 qemu-system-aarch64
# macOS:          brew install qemu
# Arch:           sudo pacman -S qemu-system-x86 qemu-system-aarch64
```

On Ubuntu, `qemu-system-aarch64` is in the `qemu-system-arm` package.

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

```sh
cargo run -- aarch64
```

Builds the kernel for `aarch64-unknown-none-softfloat` and starts
`qemu-system-aarch64` on the `virt` board (`-cpu cortex-a72`, GICv2,
serial on stdio). There is no framebuffer on this path yet; the hello
string is the serial output. Ctrl-A X leaves `-nographic` QEMU, or the
kernel calls PSCI SYSTEM_OFF after printing (QEMU exits).

To only build the x86_64 images:

```sh
cargo build
```

Copies also land at:

```
target/bios.img
target/uefi.img
```

Boot a built x86 image yourself with:

```sh
qemu-system-x86_64 -drive format=raw,file=target/bios.img -serial stdio
```

The AArch64 ELF is at:

```
target/aarch64-unknown-none-softfloat/debug/kernel
```

Release build:

```sh
cargo run --release
cargo run --release -- aarch64
```

Headless checks (what CI runs):

```sh
cargo run -- --ci
cargo run -- uefi --ci
cargo run -- aarch64 --ci
```

BIOS/UEFI boots with `-display none`, require `Hello from myos`, `heap ok`,
`int ok`, and `mod ok` on serial, and expect QEMU to exit via `isa-debug-exit`
(BIOS ~20s, UEFI 60s). AArch64 requires the same four strings and exits
via PSCI SYSTEM_OFF (must not hang; CI times out at 60s).

## Modules

There is still no filesystem. "Modular" here means **runtime load of an
ELF already sitting in memory**, not Multiboot modules and not an initrd.

A module is a `#![no_std]` crate that exports:

```rust
unsafe extern "C" fn module_init(api: *const KernelApi) -> i32
unsafe extern "C" fn module_exit() // optional
```

`KernelApi` is a `#[repr(C)]` table of function pointers (`write_str`,
`alloc`, `dealloc`) defined in `modules/abi`. The kernel fills it in and
passes `&KernelApi` into `module_init`. Modules must not call kernel
internals; they only go through that table. There is no dynamic linker
that resolves against the kernel `.dynsym`.

The hello module (`modules/hello`) prints `mod ok` through `write_str`.
It is its own tiny cargo workspace (`panic = "abort"`). `kernel/build.rs`
builds it for the kernel's target into `OUT_DIR` and feeds the ELF path
into `include_bytes!`. That avoids cargo artifact-deps (the kernel is
already an artifact of the host crate; nesting another one panics the
feature resolver) and `[build-dependencies]` (those cannot `panic=abort`).
After heap and IRQs are up, the loader copies `PT_LOAD` into the heap,
applies relocs, finds `module_init` in `.symtab`, and calls it.

x86_64 hello is a PIE (`ET_DYN`) with `R_X86_64_RELATIVE` relocs.
AArch64 hello is `ET_EXEC` slid as a unit: prebuilt `libcore` is not PIC,
so `-pie` fails to link (`R_AARCH64_ABS64` in libcore). `module_init` uses
PC-relative `ADR`, so a slide is enough. Both images use 4 KiB
`max-page-size` so they fit on the 128 KiB heap.

### Adding another module

1. Copy `modules/hello` to `modules/foo` (keep `myos-abi`, `module_init`,
   a `_start` stub, a panic handler, `[workspace]`, and `panic = "abort"`).
2. In `kernel/build.rs`, cargo-build `foo` the same way as hello, then
   `include_bytes!(env!("FOO_MODULE_PATH"))` and
   `modules::load("foo", FOO_IMAGE)` after the heap exists.
3. Keep `opt-level = "s"`, `debug = false`, `strip = "debuginfo"` so the
   ELF stays small. Use `-u module_init` (see `modules/hello/build.rs`)
   instead of `--export-dynamic`.

## Layout

| Path | Role |
|------|------|
| `src/main.rs` | Host launcher: starts QEMU (BIOS, UEFI, AArch64) |
| `build.rs` | `DiskImageBuilder` → BIOS + UEFI images |
| `kernel/src/main.rs` | `no_std` entry: hello, heap, timer IRQ, load module, halt |
| `kernel/src/heap.rs` | 128 KiB `linked_list_allocator` heap |
| `kernel/src/modules/` | ELF64 loader, `KernelApi` wrappers, loaded-module registry |
| `modules/abi` | Shared `KernelApi` / `module_init` C ABI |
| `modules/hello` | Sample module; embedded into the kernel at build time |
| `kernel/src/arch/x86/` | COM1, paging, GDT/IDT/PIC/PIT, isa-debug-exit |
| `kernel/src/arch/aarch64/` | `_start`, PL011, identity MMU, GICv2 timer, PSCI off |
| `kernel/src/framebuffer.rs` | Pixel writer for the x86_64 bootloader framebuffer |
| `kernel/src/font.rs` | Tiny 8x8 bitmap font |
| `.cargo/config.toml` | `bindeps` (artifact dependencies) |
| `rust-toolchain.toml` | pinned nightly + `llvm-tools-preview` + rust-src + targets |

## Notes

- On x86_64 the CPU is halted with `hlt` after printing (QEMU stays open
  unless you pass `--ci`, which attaches `isa-debug-exit` so QEMU exits).
- On AArch64 the kernel issues PSCI `SYSTEM_OFF` after printing, which
  QEMU treats as a shutdown.
- QEMU `virt` RAM starts at `0x40000000`. The DTB can sit there on a
  bare-metal boot, so the kernel is linked at `0x40080000`. UART0 is the
  virt PL011 at `0x09000000`.
- Paging: x86_64 keeps the bootloader's offset map and maps a 128 KiB
  heap at `0x_4444_4444_0000`. AArch64 identity-maps 1 GiB of devices +
  1 GiB of RAM and turns the EL1 MMU on.
- Interrupts: x86_64 uses the 8259 PIC + PIT (IRQ0). AArch64 uses GICv2
  (`-machine virt,gic-version=2`) and the generic physical timer (PPI 30).
- Modules run from the heap (x86_64 heap PTEs are executable; AArch64 RAM
  is EL1-executable). The loader flushes the I-cache on AArch64 after
  copying.
