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

The kernel runs round-robin **kernel threads** plus one **user `init` process**.
Userspace is a real `#![no_std]` ELF (`user/init`), not a handwritten blob:
nested cargo like hello, baked in with `include_bytes!`, loaded into
per-process page tables at `USER_BASE`, write/exit syscalls. It is not a
kernel module (no `KernelApi`). Init has its own CR3/TTBR0; the kernel/HHDM
(and on AArch64 the TTBR0 device block for UART/GIC) is mapped into the
aspace. It drops to ring 3 / EL0 and uses `syscall` / `svc` for write and
exit. Kernel threads can `task::yield_now()` cooperatively; the timer IRQ
also calls the same `task::schedule()` after EOI, so the switch is
preemptive too (including user mode). CI checks `task a`, `task b`,
`sched ok`, and `user ok`.

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
AAVMF/QEMU_EFI. Serial is on stdio; ramfb gives QEMU a graphical window.
CI stays serial-only. Close the window or wait for PSCI SYSTEM_OFF.

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
`int ok`, `task a`, `task b`, `sched ok`, `mod ok`, `limine mod ok`, and
`user ok` on serial, and expect QEMU to exit via `isa-debug-exit`
(BIOS ~20s, UEFI 60s). AArch64 requires the same strings and exits
via PSCI SYSTEM_OFF (must not hang; CI times out at 60s).

## Modules

There is still no filesystem. A module is an ELF the kernel already has in
RAM. **One loader** (`kernel/src/modules`) copies `PT_LOAD`, applies relocs,
and calls `module_init`. The two names below are only **how the bytes got
into RAM**:

| | Embedded | Limine |
|---|---|---|
| Bytes live in | the kernel binary (`include_bytes!`) | a file on the ESP (`boot/foo`) |
| Who maps them | the compiler | Limine, from `module_path` in `limine.conf` |
| Kernel hook | `modules::load("foo", FOO_IMAGE)` | `modules::load_limine_modules()` (already walks every Limine module) |
| Rebuild | kernel | disk image (`bios.img` / `uefi.img` / `aarch64.img`) |
| Hello today | yes (`mod ok`) | yes (`mod ok`, then kernel prints `limine mod ok`) |

A module is a `#![no_std]` crate that exports:

```rust
unsafe extern "C" fn module_init(api: *const KernelApi) -> i32
unsafe extern "C" fn module_exit() // optional
```

`KernelApi` (`modules/abi`) is a `#[repr(C)]` table (`write_str`, `alloc`,
`dealloc`). The kernel fills it and passes `&KernelApi` into `module_init`.
Modules must not call kernel internals. There is no dynamic linker against
kernel `.dynsym`.

Do **not** add a module as a cargo artifact-dep of the kernel (that panics
the feature resolver). Do **not** put it in `[build-dependencies]` (those
cannot `panic=abort`). Each module crate is its own tiny workspace, like
`modules/hello`.

x86_64 modules are PIE (`ET_DYN`, `R_X86_64_RELATIVE`). AArch64 modules are
`ET_EXEC` slid as a unit: prebuilt `libcore` is not PIC, so `-pie` fails to
link. `module_init` uses PC-relative `ADR`. Both use 4 KiB `max-page-size`
so they fit on the 256 KiB heap.

### 1. The ELF crate (both paths)

Copy `modules/hello` to `modules/foo`. Keep:

- `[workspace]` and `panic = "abort"` in `Cargo.toml` (`opt-level = "s"`,
  `debug = false`, `strip = "debuginfo"`)
- `myos-abi = { path = "../abi" }`
- `module_init` / optional `module_exit`, a `_start` stub, a panic handler
- the link flags in `modules/hello/build.rs` (`-z max-page-size=4096`,
  `-u module_init` / `-u module_exit`, and `-pie -nostdlib` on x86 only;
  never `--export-dynamic`)

Then choose embedded, Limine, or both (hello uses both).

### 2. Embedded: bake it into the kernel

The kernel build script compiles the crate and the ELF becomes part of the
kernel image. No ESP file, no `limine.conf` line.

1. In `kernel/build.rs`, nested-`cargo build` `modules/foo` the same way as
   hello: own `--target-dir` under `OUT_DIR`, `--target $TARGET`,
   `RUSTFLAGS=-C panic=abort`. `cargo:rerun-if-changed` its sources.
2. Point the kernel at that ELF:
   `println!("cargo:rustc-env=FOO_MODULE_PATH={}", elf.display());`
3. In `kernel/src/modules/mod.rs`:
   `const FOO_IMAGE: &[u8] = include_bytes!(env!("FOO_MODULE_PATH"));`
4. After heap and IRQs are up (see `kernel/src/main.rs`):
   `modules::load("foo", FOO_IMAGE);`

That is exactly `load_embedded_hello()` for `modules/hello`.

### 3. Limine: put it on the ESP

Limine loads extra files at boot and hands the kernel a pointer + size.
The kernel still uses the same ELF loader. You do **not** need a new
`include_bytes!` if you only want the Limine path: `load_limine_modules()`
already iterates every module Limine listed.

1. Build the ELF the same way as hello. `kernel/build.rs` also copies hello
   to a stable path the host can find:
   `target/hello-x86_64-unknown-none` and
   `target/hello-aarch64-unknown-none-softfloat`.
2. Write those bytes onto the ESP as `boot/foo`.
   `write_esp_image` in `src/limine_image.rs` already does this for hello
   (`boot/hello`). Host `build.rs` (x86) and `build_aarch64_image` in
   `src/main.rs` pass the file in.
3. Add a line under `/myos` in `LIMINE_CONF` (`src/limine_image.rs`):

   ```
   module_path: boot():/boot/foo
   ```

   Hello’s line is `module_path: boot():/boot/hello`. You can repeat
   `module_path` for more files.
4. Reboot. On success the kernel prints `limine mod ok` (hello’s own
   `module_init` still prints `mod ok`).

Changing only a Limine module does not require a new kernel `include_bytes!`,
but you still rebuild the **disk image** so the ESP file updates.

## Userspace

Userspace is a real `init` ELF (`user/init`), not a handwritten machine-code
blob. The crate is its own tiny workspace (same shape as `modules/hello`:
`panic = "abort"`, `opt-level = "s"`). `kernel/build.rs` nested-`cargo
build`s it; the kernel bakes the ELF with `include_bytes!` and loads
`PT_LOAD` into per-process page tables at `USER_BASE`, applying the same
relocs as the module loader. Init is **not** a kernel module: no
`KernelApi`, no `module_init`. It issues write/exit syscalls (`user ok`).

## Layout

| Path | Role |
|------|------|
| `src/main.rs` | Host launcher: starts QEMU (BIOS, UEFI, AArch64) |
| `src/limine_image.rs` | GPT+FAT ESP writer + Limine binary fetch + `limine.conf` |
| `build.rs` | Fetch Limine, wrap the x86_64 kernel in BIOS+UEFI images |
| `kernel/src/main.rs` | `no_std` Limine entry: hello, heap, timer IRQ, kernel threads, modules, user init, halt |
| `kernel/src/limine_boot.rs` | Limine requests (HHDM, memmap, DTB, FB, modules, executable addr) |
| `kernel/src/mm.rs` | Physical frame bump after the 256 KiB heap (page tables, user pages) |
| `kernel/src/user.rs` | Load `user/init` ELF at `USER_BASE`, per-process page tables, `syscall`/`svc` write+exit, enter ring 3 / EL0 |
| `kernel/link.ld` | Higher-half (`0xffffffff80000000`) linker script |
| `kernel/src/heap.rs` | 256 KiB `linked_list_allocator` heap from Limine usable+HHDM |
| `kernel/src/task/` | Round-robin kernel threads + user tasks: `yield_now` + timer preemption |
| `kernel/src/modules/` | ELF64 loader, `KernelApi` wrappers, loaded-module registry |
| `modules/abi` | Shared `KernelApi` / `module_init` C ABI |
| `modules/hello` | Sample module: embedded **and** ESP `boot/hello` via Limine |
| `user/init` | Userspace init ELF: prints `user ok` then exits (not a kernel module) |
| `kernel/src/arch/x86/` | COM1, GDT (user segs)/TSS RSP0/IDT/xAPIC, isa-debug-exit |
| `kernel/src/arch/aarch64/` | PL011, TTBR0 device map, GICv2 timer, lower-EL SVC, PSCI off |
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
