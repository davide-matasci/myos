# myos

A small, readable kernel in Rust. It boots in QEMU and prints
`Hello from myos` on serial (and to a framebuffer when Limine provides one).
This is a starting point to grow into a real OS, not a feature dump.

Both x86_64 and AArch64 boot through **[Limine](https://github.com/limine-bootloader/limine)**
(protocol base revision 6, `limine` crate 0.6.5). The host crate fetches a
pinned Limine binary release (`v12.6.1`), builds a GPT+FAT ESP with
`BOOTX64.EFI` / `BOOTAA64.EFI`, `limine.conf`, and the kernel ELF, and (on
x86) runs `limine bios-install` so the same image is BIOS+UEFI.

There is no rust-osdev `bootloader` 0.11 path and no QEMU `-kernel` stub.
Multiboot is not used (the spec is i386/MIPS32; there is no merged
Multiboot-for-ARM).

Nightly is **pinned** (`nightly-2026-07-26`). Do not unpin it in this pass.

## Prerequisites

- [rustup](https://rustup.rs/) (the `rust-toolchain.toml` in this repo
  selects the pinned nightly and the components below)
- QEMU (`qemu-system-x86_64` and, for AArch64, `qemu-system-aarch64`)
- A C compiler (`cc`) to build the Limine host tool (`bios-install`)
- `curl` to fetch the pinned Limine binary tarball on first build
- For AArch64: UEFI firmware (the launcher tries `ovmf-prebuilt`, then
  distro AAVMF paths such as `/usr/share/AAVMF/AAVMF_CODE.fd`)

Nightly components / targets (installed automatically by rustup from
`rust-toolchain.toml`):

- `llvm-tools-preview`
- `rust-src`
- `x86_64-unknown-none` — x86_64 kernel target
- `x86_64-unknown-uefi` — unused by the kernel now, kept for toolchain stability
- `aarch64-unknown-none-softfloat` — AArch64 kernel target (softfloat so we
  do not touch NEON/FP before the CPU is set up)

```sh
# rustup reads rust-toolchain.toml on the first cargo invocation
# and installs the pinned nightly + components + targets.

# Debian/Ubuntu:  sudo apt install qemu-system-x86 qemu-system-arm qemu-efi-aarch64 gcc
# Fedora:         sudo dnf install qemu-system-x86 qemu-system-aarch64 edk2-aarch64 gcc
# macOS:          brew install qemu
# Arch:           sudo pacman -S qemu-system-x86 qemu-system-aarch64 gcc
```

On Ubuntu, `qemu-system-aarch64` is in the `qemu-system-arm` package and
AArch64 UEFI firmware is `qemu-efi-aarch64`.

## Build and run

From the repo root:

```sh
cargo run
```

That compiles the kernel for `x86_64-unknown-none`, wraps it in a Limine
GPT+FAT disk (BIOS stages installed), and starts QEMU. You should see a
green `Hello from myos` on a black screen when a framebuffer is present.
Close the QEMU window to exit.

```sh
cargo run -- uefi
```

Same x86_64 image over UEFI (OVMF firmware is fetched on first run into
`target/ovmf`).

```sh
cargo run -- aarch64
```

Builds the kernel for `aarch64-unknown-none-softfloat`, writes
`target/aarch64.img` (ESP with `BOOTAA64.EFI`), and starts
`qemu-system-aarch64` on `virt,gic-version=2` (`-cpu cortex-a72`) with
AAVMF/QEMU_EFI. Serial is on stdio. Ctrl-A X leaves `-nographic` QEMU, or
the kernel calls PSCI SYSTEM_OFF after printing (QEMU exits).

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
qemu-system-x86_64 -m 256 -drive format=raw,file=target/bios.img -serial stdio
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
`int ok`, `mod ok`, and `limine mod ok` on serial, and expect QEMU to exit via `isa-debug-exit`
(BIOS ~20s, UEFI 60s). AArch64 requires the same five strings and exits
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
Hello is both baked into the kernel and loaded from the ESP via Limine
`module_path`.

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
| `src/limine_image.rs` | GPT+FAT ESP writer + Limine binary fetch |
| `build.rs` | Fetch Limine, wrap the x86_64 kernel in BIOS+UEFI images |
| `kernel/src/main.rs` | `no_std` Limine entry: hello, heap, timer IRQ, load module, halt |
| `kernel/src/limine_boot.rs` | Limine requests (base rev, HHDM, memmap, DTB, FB, executable addr) |
| `kernel/link.ld` | Higher-half (`0xffffffff80000000`) linker script |
| `kernel/src/heap.rs` | 128 KiB `linked_list_allocator` heap from Limine usable+HHDM |
| `kernel/src/modules/` | ELF64 loader, `KernelApi` wrappers, loaded-module registry |
| `modules/abi` | Shared `KernelApi` / `module_init` C ABI |
| `modules/hello` | Sample module; embedded into the kernel at build time |
| `kernel/src/arch/x86/` | COM1, GDT/IDT/PIC/PIT, isa-debug-exit |
| `kernel/src/arch/aarch64/` | PL011, TTBR0 device map, GICv2 timer, PSCI off |
| `kernel/src/framebuffer.rs` | Pixel writer for a Limine framebuffer |
| `kernel/src/font.rs` | Tiny 8x8 bitmap font |
| `.cargo/config.toml` | `bindeps` (artifact dependencies) |
| `rust-toolchain.toml` | pinned nightly + `llvm-tools-preview` + rust-src + targets |

## Notes

- On x86_64 the CPU is halted with `hlt` after printing (QEMU stays open
  unless you pass `--ci`, which attaches `isa-debug-exit` so QEMU exits).
- On AArch64 the kernel issues PSCI `SYSTEM_OFF` after printing (HVC at
  EL1, SMC at EL2), which QEMU treats as a shutdown.
- The kernel is linked in the higher half. Limine sets the stack, enables
  the MMU, and provides an HHDM. Usable memory is accessed as `phys + HHDM`.
- AArch64 device MMIO (PL011 `0x09000000`, GICv2 `0x08000000`/`0x08010000`)
  is not in the HHDM at base revision 3+, so the kernel identity-maps a 1 GiB
  device block on `TTBR0`.
- Interrupts: x86_64 uses the 8259 PIC + PIT (IRQ0). AArch64 uses GICv2
  (`-machine virt,gic-version=2`) and the generic physical timer (PPI 30).
- Modules run from the HHDM heap (Limine HHDM mappings are rwx). The loader
  flushes the I-cache on AArch64 after copying.
- Limine binaries are downloaded from GitHub release `v12.6.1` (sha256-pinned)
  into `target/limine-v12.6.1`. Not a git submodule.
