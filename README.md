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

The kernel runs round-robin **kernel threads** plus **user processes**.
Init (`user/init`) is PID1-style: a real `#![no_std]` ELF, baked in with
`include_bytes!`, spawned as today, then **execs `/sh`**. The shell prints
`sh ok`, smoke-runs `/ok` (fork/exec with argv), then drops to an
interactive `$` prompt on **stdin** (PS/2 keyboard when detected, else
serial). `user/ok` prints `user ok`, reads
`/msg` (FAT16 `MSG` via virtio-blk), prints `fat ok`, and exits.
Userspace programs are ELFs, not `KernelApi` modules. Nested cargo like
hello; loaded into per-process page tables at `USER_BASE`. Each process has
its own CR3/TTBR0; the kernel/HHDM (and on AArch64 the TTBR0 device block
for UART/GIC) is mapped into the aspace. It drops to ring 3 / EL0 and uses
`syscall` / `svc`. Kernel threads can `task::yield_now()` cooperatively;
the timer IRQ also calls the same `task::schedule()` after EOI, so the
switch is preemptive too (including user mode). CI checks `task a`,
`task b`, `sched ok`, `sh ok`, `user ok`, and `fat ok`.

## Prerequisites

- [rustup](https://rustup.rs/) (the `rust-toolchain.toml` in this repo
  selects the pinned nightly and the components below)
- QEMU (`qemu-system-x86_64` and, for AArch64, `qemu-system-aarch64`)
- A C compiler (`cc`) to build the Limine host tool (`bios-install`)
- `curl` to fetch the pinned Limine binary tarball on first build
- `xorriso` to write the hybrid ISO (`cargo run -- iso` only; Debian package `xorriso`)
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
# ISO only:       sudo apt install xorriso
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
GPT+FAT disk (BIOS stages installed), writes `target/fat.img` (FAT16 data
disk), and starts QEMU with a second virtio-blk device. You should see a
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
`target/aarch64.img` (ESP with `BOOTAA64.EFI`) and `target/fat.img`, and
starts `qemu-system-aarch64` on `virt,gic-version=2` (`-cpu cortex-a72`)
with AAVMF/QEMU_EFI. Serial is on stdio; ramfb gives QEMU a graphical
window. CI stays serial-only. Close the window or wait for PSCI SYSTEM_OFF.

To only build the x86_64 images:

```sh
cargo build
```

Copies also land at:

```
target/bios.img
target/uefi.img
target/fat.img
```

Boot a built x86 image yourself with:

```sh
qemu-system-x86_64 -m 256 \
  -drive format=raw,file=target/bios.img \
  -drive if=none,id=vd0,format=raw,file=target/fat.img \
  -device virtio-blk-pci,drive=vd0,disable-modern=on \
  -serial stdio
```

Write a BIOS+UEFI hybrid ISO (needs `xorriso`; does not start QEMU):

```sh
cargo run -- iso
```

That writes `target/myos-x86_64.iso`. GitHub Actions → ISO → Run workflow
uploads the same artifact. It does not run on push. Optional QEMU:

```sh
qemu-system-x86_64 -m 256 -cdrom target/myos-x86_64.iso -serial stdio
```

`target/fat.img` is still a separate virtio-blk disk; attach it as in the
BIOS command above if you want `/msg`.

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
`int ok`, `task a`, `task b`, `sched ok`, `mod ok`, `limine mod ok`,
`sh ok`, `user ok`, and `fat ok` on serial. CI kills QEMU once all needles
are seen (the shell would otherwise block on stdin). AArch64 requires the
same strings and exits the same way (CI times out at 60s if needles are
missing).

Interactive use (keyboard on x86 in QEMU window, or serial on a TTY):

```sh
cargo run          # x86 BIOS — type at `$` (keyboard or serial)
cargo run -- uefi
cargo run -- aarch64   # serial only for now
```

## Real hardware (not QEMU)

On a physical PC you usually see Limine and then **green text on the
monitor**. The kernel mirrors serial output to that framebuffer, so boot
progress (`heap ok`, `kbd ok`, `sh ok`, `$`, …) should scroll on screen as
well as on serial.

**stdin (fd 0)** merges **PS/2 keyboard** (x86, when the 8042 responds) and
**serial**. If probe succeeds the kernel prints `kbd ok` and you can type at
`$` on a directly attached keyboard (USB keyboards usually work in legacy
PS/2 mode). Serial remains available at the same time.

If no keyboard is detected, use serial only:

| Arch | Port | Settings |
|------|------|----------|
| x86_64 | **COM1** (I/O `0x3F8`) | **38400** 8N1 |
| AArch64 | PL011 UART (board-specific; `0x09000000` on QEMU virt) | 115200 8N1 typical |

On many desktops COM1 is a motherboard header (9-pin) or a rear DB9. Use a
USB‑serial adapter, connect **GND/RX/TX** (often a null-modem cable to a
second PC), and open the port in `minicom`, `picocom`, or PuTTY at the baud
above. Example:

```sh
picocom -b 38400 /dev/ttyUSB0
```

Write the Limine disk image to a USB stick or internal drive the same way
you would for QEMU (`target/bios.img` or UEFI ESP). Attach a **second**
FAT16 data disk only if you want `/msg` / `fat ok` (virtio-blk is expected;
bare metal without that disk still boots the shell).

If the **monitor stops at `Hello from myos`** on an old image, the OS was
still running on serial only — update to a build with framebuffer mirroring
or connect serial to see the rest.

## VFS, virtio-blk, and FAT16

A tiny VFS (`kernel/src/fs/`) is a **mount table** with ops/`lookup`. The
backend is **bootfs**: Limine modules (basename of `file.path()`, stripping
`boot():` and slashes), an embedded `/ok` ELF fallback, and files
registered at runtime through `KernelApi::vfs_register`. ESP `boot/ok`
overwrites the embed when both exist.

**virtio-blk is in-kernel** (`kernel/src/blk.rs`), not a loadable module
(chicken/egg: the FAT parser needs block I/O to load). x86 uses PCI config
(`0xCF8`/`0xCFC`) to find vendor `0x1AF4` device `0x1001` and talks
**legacy I/O-BAR** virtio-blk (QEMU `disable-modern=on`). AArch64 probes
**virtio-mmio v2** at `0x0a000000` (stride `0x200`, device id 2) for
`-device virtio-blk-device`. Guest DMA addresses are HHDM VA minus
`hhdm_offset()` (frame phys from `mm::alloc_frame`), not
`kernel_virt_to_phys`.

The second QEMU disk is `target/fat.img` (~20 MiB raw FAT16 so the
existing `format_and_write_fat16` writer stays in the FAT16 cluster
range). It is **not** the boot drive. All launcher QEMU invocations add:

x86:

```
-drive if=none,id=vd0,format=raw,file=<fat.img>
-device virtio-blk-pci,drive=vd0,disable-modern=on
```

aarch64:

```
-drive if=none,id=vd0,format=raw,file=<fat.img>
-device virtio-blk-device,drive=vd0
```

**FAT16 is a kernel module** (`modules/fat`), same shape as hello, loaded
from an embed after virtio init. It uses KernelApi only (`blk_read` /
`vfs_register`): parse the BPB, scan the root directory for `MSG`, walk
the FAT16 cluster chain, and register `/msg`. Root-only, no subdirs, no
FAT32. This is **not** FUSE or a userspace FAT parser, and virtio is
**not** in a module.

## Modules

Kernel modules are still ELFs the kernel already has in RAM (not loaded
through open/exec). **One loader** (`kernel/src/modules`) copies `PT_LOAD`,
applies relocs, and calls `module_init`. `/hello` is still the kernel hello
module. The two names below are only **how the bytes got into RAM**:

| | Embedded | Limine |
|---|---|---|
| Bytes live in | the kernel binary (`include_bytes!`) | a file on the ESP (`boot/foo`) |
| Who maps them | the compiler | Limine, from `module_path` in `limine.conf` |
| Kernel hook | `modules::load("foo", FOO_IMAGE)` | `modules::load_limine_modules()` (already walks every Limine module) |
| Rebuild | kernel | disk image (`bios.img` / `uefi.img` / `aarch64.img`) |
| Hello today | yes (`mod ok`) | yes (`mod ok`, then kernel prints `limine mod ok`) |
| FAT16 today | yes (`modules/fat`, registers `/msg`) | no (embed is enough; a Limine copy would print `limine mod ok` twice) |

A module is a `#![no_std]` crate that exports:

```rust
unsafe extern "C" fn module_init(api: *const KernelApi) -> i32
unsafe extern "C" fn module_exit() // optional
```

`KernelApi` (`modules/abi`) is a `#[repr(C)]` table (`write_str`, `alloc`,
`dealloc`, `blk_read`, `vfs_register`). ABI version is **2**; new pointers
are appended, never reordered. The kernel fills it and passes `&KernelApi`
into `module_init`. Modules must not call kernel internals. There is no
dynamic linker against kernel `.dynsym`. `blk_read` returns 0 or −1.
`vfs_register` copies into the bootfs table (kernel-owned `'static`) and
takes a basename without slash (`msg`).

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

That is exactly `load_embedded_hello()` for `modules/hello`, and
`load_embedded_fat()` for `modules/fat` (after `blk::init()`).

### 3. Limine: put it on the ESP

Limine loads extra files at boot and hands the kernel a pointer + size.
The kernel still uses the same ELF loader. You do **not** need a new
`include_bytes!` if you only want the Limine path: `load_limine_modules()`
already iterates every module Limine listed. Userspace ELFs in that list
(`MissingInit`) are skipped so they can live on bootfs instead.

1. Build the ELF the same way as hello. `kernel/build.rs` also copies hello
   to a stable path the host can find:
   `target/hello-x86_64-unknown-none` and
   `target/hello-aarch64-unknown-none-softfloat`.
2. Write those bytes onto the ESP as `boot/foo`.
   `write_esp_image` in `src/limine_image.rs` already does this for hello
   (`boot/hello`) and `ok` (`boot/ok`). Host `build.rs` (x86) and
   `build_aarch64_image` in `src/main.rs` pass the files in.
3. Add a line under `/myos` in `LIMINE_CONF` (`src/limine_image.rs`):

   ```
   module_path: boot():/boot/foo
   ```

   Hello’s line is `module_path: boot():/boot/hello`. `ok` is
   `module_path: boot():/boot/ok`. You can repeat `module_path` for more
   files. FAT 8.3: keep names short (`ok`, `hello`) so they are not LFN.
4. Reboot. On success the kernel prints `limine mod ok` (hello’s own
   `module_init` still prints `mod ok`).

Changing only a Limine module does not require a new kernel `include_bytes!`,
but you still rebuild the **disk image** so the ESP file updates.

## Userspace

Userspace programs are ELFs, not KernelApi modules. Init is PID1-style:
baked into the kernel (`user/init`), spawned as today, and **execs `/sh`**.
The shell (`user/sh`) is a tiny `#![no_std]` program: it smoke-runs `/ok`
(fork/exec with argv) for CI, then reads lines from **stdin (fd 0)** and
fork/exec's built-in utilities (`echo`, `cat`, `ls`, `ok`, …). Shared
helpers live in `user/lib` (`myos_user`: syscalls, argv, `read_line`,
`listdir`, `brk`, bump [`Heap`](user/lib/src/alloc.rs)).

`user/ok` is its own tiny workspace (same shape as `user/init` /
`modules/hello`: `panic = "abort"`, `opt-level = "s"`). `kernel/build.rs`
nested-`cargo build`s init, sh, ok, echo, cat, ls; init is `include_bytes!`,
the rest are embedded as bootfs fallbacks and placed on the ESP as
`boot/sh`, `boot/ok`, etc. After printing `user ok`, it `open`s `/msg`,
`read`s the bytes, writes them to serial (`fat ok`), and exits. If `/msg`
is missing it spins (CI then fails the `fat ok` needle).

`PT_LOAD` is realized at `USER_BASE` with the same relocs as the module
loader. **fork** copies user pages into a new aspace (no COW) and a new
task; the parent gets the child pid (task slot), the child gets 0.
**exec** replaces the calling task's image (no second `USERS_ALIVE` /
`note_exit`) and accepts an optional argv pack. **wait** reaps a zombie
child. Syscall numbers are **append-only** (reserved for future libc /
compiler ports). Errors return `usize::MAX`.

| nr | name | args |
|----|------|------|
| 0 | write | ptr, len |
| 1 | exit | |
| 2 | open | path, path_len → fd (≥3) |
| 3 | read | fd, buf, len → n (fd 0 = keyboard + serial stdin) |
| 4 | close | fd |
| 5 | exec | path, path_len, args_ptr (0 or `[argc, (ptr,len)...]`) |
| 6 | fork | → child pid (parent), 0 (child) |
| 7 | wait | → reaped child pid |
| 8 | listdir | buf, len → byte count (bootfs names, newline-separated) |
| 9 | brk | addr → program break (0 = query). Per-process heap after stack (256 KiB max) |

x86 `syscall`: `rax`=nr, `rdi`/`rsi`/`rdx`=a0/a1/a2. At `_start`, argc/argv
are on the user stack (System V). AArch64 `svc`: `x8`=nr, `x0`/`x1`/`x2`=a0/a1/a2.
At `_start`, the kernel passes **argc in x0, argv in x1** (argv points to
an array of string pointers). Exec argv strings must live in writable user
memory (stack); rodata literals are rejected by the kernel copy-in path.

### Userspace heap and `std` bring-up

Syscall **9 (`brk`)** backs a per-process heap region above the stack page.
`user/lib` exposes `brk`, `heap_init`, and a bump [`GlobalAlloc`](user/lib/src/alloc.rs)
(`myos_user::Heap`). The `user/heap` ELF smoke-tests it (`alloc ok` on serial).

To port **`std`** (see [OSDev](https://wiki.osdev.org/Porting_Rust_standard_library)),
copy the in-repo PAL skeleton into a patched Rust tree:

| Path | Role |
|------|------|
| `targets/x86_64-unknown-myos.json` | Custom userspace triple (`os = "myos"`) |
| `std/pal/myos/` | PAL files → `library/std/src/sys/pal/myos/` in rustc |
| `std/pal/README.md` | `build.rs` / `pal/mod.rs` patch steps, `-Z build-std` |

Full `std` still needs more syscalls (`mmap`, time, …); the skeleton targets
`println!("std ok")` after patching the pinned nightly.

## Layout

| Path | Role |
|------|------|
| `src/main.rs` | Host launcher: starts QEMU (BIOS, UEFI, AArch64) + second virtio-blk disk |
| `src/limine_image.rs` | GPT+FAT ESP writer + Limine binary fetch + `limine.conf` + `fat.img` |
| `build.rs` | Fetch Limine, wrap the x86_64 kernel in BIOS+UEFI images, write `fat.img` |
| `kernel/src/main.rs` | `no_std` Limine entry: hello, heap, timer IRQ, kernel threads, bootfs, modules, virtio-blk, fat, user init, halt |
| `kernel/src/limine_boot.rs` | Limine requests (HHDM, memmap, DTB, FB, modules, executable addr) |
| `kernel/src/mm.rs` | Physical frame bump after the 256 KiB heap (page tables, user pages, virtqueues) |
| `kernel/src/blk.rs` | In-kernel virtio-blk: `init` + 512-byte sector `read` |
| `kernel/src/fs/` | Tiny VFS: mount table + bootfs (Limine modules + embedded `/ok` + `vfs_register`) |
| `kernel/src/console.rs` | Dual console: serial + Limine framebuffer mirror |
| `kernel/src/input.rs` | Stdin ring buffer: PS/2 keyboard + serial (fd 0) |
| `kernel/src/arch/x86/keyboard.rs` | PS/2 keyboard via 8042 (poll, US QWERTY set 1) |
| `kernel/link.ld` | Higher-half (`0xffffffff80000000`) linker script |
| `kernel/src/heap.rs` | 256 KiB `linked_list_allocator` heap from Limine usable+HHDM |
| `kernel/src/task/` | Round-robin kernel threads + user tasks: `yield_now` + timer preemption |
| `kernel/src/modules/` | ELF64 loader, `KernelApi` wrappers, loaded-module registry |
| `modules/abi` | Shared `KernelApi` / `module_init` C ABI (v2: `blk_read`, `vfs_register`) |
| `modules/hello` | Sample module: embedded **and** ESP `boot/hello` via Limine |
| `modules/fat` | FAT16 kernel module: `blk_read` + `vfs_register("msg")` from root `MSG` |
| `user/init` | PID1-style: baked in, execs `/sh` (not a kernel module) |
| `user/sh` | Minimal shell: smoke `/ok`, interactive `$` prompt on stdin |
| `user/echo` | Print argv (`echo hello`) |
| `user/cat` | Read a bootfs file to stdout |
| `user/ls` | List bootfs entries (via `listdir`) |
| `user/lib` | Shared `myos_user` syscall/argv/`Heap` helpers |
| `user/heap` | `#![no_std]` + `alloc` smoke test (`alloc ok`) |
| `user/ok` | Second userspace ELF: `user ok`, then reads `/msg`; ESP `boot/ok` |
| `targets/` | Custom Rust target specs (`x86_64-unknown-myos.json`) |
| `std/pal/` | Rust `std` PAL skeleton + porting notes |
| `kernel/src/arch/x86/` | COM1, GDT (user segs)/TSS RSP0/IDT/xAPIC, PCI, legacy virtio-blk, isa-debug-exit |
| `kernel/src/arch/x86/pci.rs` | PCI config via `0xCF8`/`0xCFC`; find virtio-blk |
| `kernel/src/arch/aarch64/` | PL011, TTBR0 device map, GICv2 timer, lower-EL SVC, virtio-mmio blk, PSCI off |
| `kernel/src/framebuffer.rs` | Pixel writer for a Limine framebuffer |
| `kernel/src/font.rs` | Tiny 8x8 bitmap font |
| `.cargo/config.toml` | `bindeps` (artifact dependencies) |
| `rust-toolchain.toml` | pinned nightly + `llvm-tools-preview` + rust-src + targets |
| `.github/workflows/iso.yml` | Manual `workflow_dispatch` x86_64 hybrid ISO artifact |

## Notes

- On x86_64 the CPU is halted with `hlt` after printing (QEMU stays open
  unless you pass `--ci`, which attaches `isa-debug-exit` so QEMU exits).
- On AArch64 the kernel issues PSCI `SYSTEM_OFF` after printing (HVC at
  EL1, SMC at EL2), which QEMU treats as a shutdown.
- The kernel is linked in the higher half. Limine sets the stack, enables
  the MMU, and provides an HHDM. Usable memory is accessed as `phys + HHDM`.
- AArch64 device MMIO (PL011 `0x09000000`, GICv2 `0x08000000`/`0x08010000`,
  virtio-mmio `0x0a000000`) is not in the HHDM at base revision 3+, so the
  kernel identity-maps a 1 GiB device block on `TTBR0`.
- Interrupts: x86_64 uses the local xAPIC timer. AArch64 uses GICv2
  (`-machine virt,gic-version=2`) and the generic physical timer (PPI 30).
- Modules run from the HHDM heap (Limine HHDM mappings are rwx). The loader
  flushes the I-cache on AArch64 after copying.
- Limine binaries are downloaded from GitHub release `v12.6.1` (sha256-pinned)
  into `target/limine-v12.6.1`. Not a git submodule.
- virtio-blk init does not panic if the device is missing (earlier CI
  needles can still print); `/msg` is then absent and CI fails on `fat ok`.
