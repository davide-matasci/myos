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

Userspace **ports** live under `ports/` (sbase, ubase, oksh, ripgrep, coreutils, tcc; fetched at build, not vendored). **Toolchain** pieces live under `toolchain/` (newlib/libgloss and the Rust `std` PAL/sysroot). `scripts/` keeps thin wrappers so `./scripts/build-sbase.sh` still works.

The kernel runs round-robin **kernel threads** plus **user processes**.
Init (`user/init`) is PID1-style: a real `#![no_std]` ELF, baked in with
`include_bytes!`, spawned as today, smoke-runs fork/`/ok` for CI needles,
then **stays PID1** and forks **getty** (`/u/getty /dev/console linux`).
Getty sets up the console, prompts `login: `, and execs **`/u/login`**.
Login is fake single-user (`root` / empty-or-any password) and execs `/sh`.
If getty/login/sh exits, init `wait()`s and respawns getty. `/sh` is portable
OpenBSD ksh ([oksh](https://github.com/ibara/oksh) 7.9) linked with
newlib/libgloss. It prints `sh ok` and drops to an interactive `$ ` prompt on
**stdin** (PS/2 keyboard when detected, else serial). Slim always-on `user/ok`
proves `alloc ok`, reads `/msg` (`user ok` / `fat ok`), cheap disk/FAT listdir
markers and `/proc/mounts` (`proc ok`), then exits — it no longer execs the
heavy carnival. CI types `root`
at `login: `, an empty password, then `heap` at `$` for
std/C/sbase/uutils/ripgrep/tcc/bigalloc.
`/c/rg` is BurntSushi ripgrep (fetched at build time, with PCRE2). `/t/tcc` is TinyCC (fetched at build time) with mmap-backed `-run`. Userspace programs are ELFs,
not `KernelApi` modules. Nested cargo like
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
- QEMU (`qemu-system-x86_64`, `qemu-system-aarch64`, and `qemu-system-misc` for RISC-V)
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

```sh
cargo run -- riscv64
```

Builds the kernel for `riscv64imac-unknown-none-elf`, writes
`target/riscv64.img` (ESP with `BOOTRISCV64.EFI`, Limine `global_dtb`, Sv39),
and starts `qemu-system-riscv64` on `virt` with RISC-V UEFI firmware
(`qemu-efi-riscv64` or `edk2-riscv64`). Serial is on stdio; ramfb is enabled
for a graphical window. Userspace fork/exec is working; the full CI needle
set (`user ok`, `fat ok`, interactive shell) is still being brought up on
RISC-V.

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
qemu-system-x86_64 -m 256 \\
  -drive format=raw,file=target/bios.img \\
  -drive if=none,id=vd0,format=raw,file=target/fat.img \\
  -device virtio-blk-pci,drive=vd0,disable-modern=on \\
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

A VFS layer (`kernel/src/fs/vfs.rs`) holds a **mount table**; each mount
has a name (fstype), optional path prefix, a source (`none` or a `/dev/vd*`
path), and [`MountOps`] (`lookup`, `stat`, `listdir`, `register`). Syscalls
route through the VFS, which picks the longest matching prefix (root mount
uses `""` today, so `/ok` and `ok` both resolve on bootfs). `vfs::mounts_text()`
snapshots the table as Linux-shaped `source target fstype opts 0 0` lines.

**procfs** (`kernel/src/fs/procfs.rs`) is a read-only generated mount at
`/proc`. The only node today is `/proc/mounts` (not stored bytes): `open` /
`read` / `cat /proc/mounts` print the current table. `/mount` with no
arguments is a pretty-printer of that file; `mount <source> <target> <fstype>`
still issues `SYS_MOUNT`.

**bootfs** is the first mount at `/` (`kernel/src/fs/bootfs.rs`): a flat
read-only namespace. Embedded user ELFs are registered at boot; Limine ESP
modules override by basename; loadable modules add files via
`KernelApi::vfs_register` on the `"bootfs"` mount (e.g. FAT registers
`msg`). ESP `boot/ok` overwrites the embed when both exist.

**Install layout:** handwritten Rust demos that would collide with ported
tool names use a `myos_` prefix on bootfs (`/myos_ls`, `/myos_echo`,
`/myos_cat`). Ported **sbase** keeps short names under `/s/` (PREFIX `/s`);
**uutils/coreutils** multicall names live under `/c/`. oksh PATH is
`/s:/c:/`, so bare `ls`/`cat` resolve to sbase (or coreutils) after the
rename. Root `listdir` also surfaces mount prefixes (`s`, `c`, …).

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

**ext2 is a kernel module** (`modules/ext2`) like FAT, using writable VFS ABI (`ModuleVfsOps` plus byte-granular `blk_read_at`/`blk_write_at`).

**virtio-net is a kernel module** (`modules/virtio_net`) like FAT/ext2: modern virtio 1.0 PCI, poll-mode RX/TX, `/dev/net0` Ethernet frames (no IP). Loaded after NVMe. QEMU adds `-netdev user,id=net0 -device virtio-net-pci,netdev=net0` on every arch (keeps `-nic none`).

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
`dealloc`, `blk_read`, `vfs_register`, `vfs_register_static`, `vfs_mount`).
ABI version is **7**; new pointers are appended, never reordered. The kernel
fills it and passes `&KernelApi` into `module_init`. Modules must not call
kernel internals. There is no dynamic linker against kernel `.dynsym`.
`blk_read` returns 0 or −1. `vfs_register` copies into the bootfs mount;
`vfs_register_static` borrows module/rodata bytes without copying.
`vfs_mount` attaches a [`ModuleVfsOps`] backend at `/prefix/…` (see
`modules/stubfs` mounting `/disk/ping`).

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
baked into the kernel (`user/init`), spawned as today, smoke-runs fork and
slim `/ok`, then **forks getty** (`/u/getty`) and `wait()`s, respawning if
it dies. Getty (ubase) prompts for a username and execs `/u/login`; login
(ubase) accepts fake `root:root` and execs `/sh`. The shell is **oksh 7.9**
(portable OpenBSD ksh) built with newlib/libgloss and embedded as bootfs
`sh`. The older tiny `user/sh` crate stays in-tree but is not `/sh`. Shared
helpers for the remaining Rust user programs live in `user/lib`.

`user/ok` is its own tiny workspace (same shape as `user/init` /
`modules/hello`: `panic = "abort"`, `opt-level = "s"`). `kernel/build.rs`
nested-`cargo build`s init, ok, heap, myos_echo, myos_cat, myos_ls (not
`sh`); init is `include_bytes!`, the rest are embedded as bootfs fallbacks
and placed on the ESP as `boot/sh`, `boot/ok`, etc. Slim `/ok` always runs
at boot: `heap_init` + `alloc ok`, `user ok`, `/msg` → `fat ok`,
`/disk/ping` → `disk ok`, cheap `listdir` → `disk ls ok` / `fat ls ok`,
and `/fat/msg` → `fat read ok`. It does **not** fork the std/C/sbase/uutils
suite. That carnival lives in bootfs `/heap` and is invoked by CI
`wait_ci` typing `heap` at the interactive `$` prompt on x86, aarch64, and riscv64. If `/msg` is missing
`/ok` exits early (CI then fails the `fat ok` needle).

`PT_LOAD` is realized at `USER_BASE` with the same relocs as the module
loader. **fork** copies user pages into a new aspace (no COW) and a new
task; the parent gets the child pid (task slot), the child gets 0.
**exec** replaces the calling task's image (no second `USERS_ALIVE` /
`note_exit`) and accepts an optional argv pack. **wait** reaps a zombie
child. Syscall numbers are **append-only** (reserved for future libc /
compiler ports). Errors return `usize::MAX`.

| nr | name | args |
|----|------|------|
| 0 | write | fd, ptr, len |
| 1 | exit | code |
| 2 | open | path, path_len, flags → fd (cwd-aware) |
| 3 | read | fd, buf, len → n (fd 0 = keyboard + serial stdin) |
| 4 | close | fd |
| 5 | exec | path, path_len, args_ptr (0 or `[argc, (ptr,len)...][envc, (ptr,len)...]`) |
| 6 | fork | → child pid (parent), 0 (child) |
| 7 | wait | status_ptr (0 = ignore) → reaped child pid; stores exit code byte if ptr set |
| 8 | listdir | path, path_len, buf → byte count (cwd-aware; newline-separated) |
| 9 | brk | addr → program break (0 = query). Per-process heap after stack (256 KiB max) |
| 10 | pipe | fds_ptr (two usize slots) → 0 or error |
| 11 | dup2 | oldfd, newfd → 0 or error |
| 12 | stat | path, path_len, out_ptr → 0 or error |
| 13 | execname | buf, len → basename length |
| 14 | dupfd | oldfd, minfd → new fd |
| 15 | chdir | path, path_len → 0 or error (per-task cwd) |
| 16 | getcwd | buf, buf_len → pathname length (NUL written) |

x86 `syscall`: `rax`=nr, `rdi`/`rsi`/`rdx`=a0/a1/a2. At `_start`, argc/argv
are on the user stack (System V). AArch64 `svc`: `x8`=nr, `x0`/`x1`/`x2`=a0/a1/a2.
At `_start`, the kernel passes **argc in x0, argv in x1** (argv points to
an array of string pointers). Exec argv strings must live in writable user
memory (stack); rodata literals are rejected by the kernel copy-in path.

### Userspace heap and `std` bring-up

Syscall **9 (`brk`)** backs a per-process heap region above the stack page.
`user/lib` exposes `brk`, `heap_init`, and a bump [`GlobalAlloc`](user/lib/src/alloc.rs)
(`myos_user::Heap`). Slim `user/ok` smoke-tests it at every boot (`alloc ok`
on serial). `user/heap` is the CI-only heavy carnival (typed at `$`).

To build **`std`** programs (see [OSDev](https://wiki.osdev.org/Porting_Rust_standard_library)):

```sh
./toolchain/std/build-std-hello.sh   # builds sysroot + smoke ELFs for x86_64 and AArch64
```

| Path | Role |
|------|------|
| `targets/*-unknown-myos.json` | Custom userspace triples (`os = "myos"`) |
| `toolchain/std/pal/myos/` | PAL → `library/std/src/sys/pal/myos/` in the patched sysroot |
| `toolchain/std/build-sysroot.sh` | Precompile `std` into `target/myos-sysroot` (both triples) |
| `toolchain/std/fetch-sysroot.sh` | Install prebuilt sysroot tarball or build locally |
| `toolchain/std/package-sysroot.sh` | Tarball the sysroot for local reuse or CI artifacts |
| `toolchain/std/check-wire.sh` | Verify PAL patches apply to pinned nightly |
| `toolchain/std/export-upstream-patch.sh` | Generate diff for rust-lang/rust submission |
| `toolchain/std/upstream/README.md` | Tier 3 upstream checklist for `target_os = "myos"` |
| `toolchain/std/toolchain/config.toml.example` | Consumer `.cargo/config.toml` template |
| `toolchain/std/pal/README.md` | Full sysroot / build docs |

CI checks `println!("std ok")` on BIOS, UEFI, and AArch64. App crates link against
the prebuilt sysroot (no `-Z build-std` on each app build). More syscalls (`open`,
process, time, …) are still needed for real programs beyond the smoke test.

### C userspace (newlib + libgloss)

C programs link against **newlib** with a myos **libgloss** port (syscall
adapters + ENOSYS stubs). Host **clang** cross-compiles; no new kernel syscalls
required beyond the existing myos ABI.

```sh
./toolchain/newlib/build.sh   # fetch newlib 4.4.0, build libc + libgloss/myos
./scripts/build-c-hello.sh  # minimal write() smoke → target/c-hello-*
./ports/sbase/build.sh    # full suckless sbase → target/sbase-* + manifest
./ports/ubase/build.sh    # ubase getty+login → target/ubase-* (`/u/…`)
./ports/oksh/build.sh     # oksh 7.9 → target/oksh-*-unknown-none (`/sh`)
./ports/tcc/build.sh      # TinyCC → target/tcc-*-unknown-myos (`/t/tcc`)
```

| Path | Role |
|------|------|
| `toolchain/newlib/libgloss/myos/` | libgloss port: `_read`/`_write`/`_open`/… → myos syscalls; stubs return `ENOSYS`/`EROFS` |
| `toolchain/newlib/fetch.sh` | Clone pinned newlib into `target/newlib-src` |
| `toolchain/newlib/patch.sh` | Register `*-unknown-myos`, install libgloss port |
| `toolchain/newlib/build.sh` | Build/install newlib per arch |
| `toolchain/newlib/build-libgloss.sh` | Build `libgloss.a` + `crt0.o` (called by build-newlib) |
| `scripts/build-c-hello.sh` | Link minimal C smoke with `-lc -lgloss` |
| `ports/sbase/fetch.sh` | Clone pinned [sbase](https://git.suckless.org/sbase) into `target/sbase-src` |
| `ports/sbase/prepare.sh` | Sync upstream tree; apply myos patches; generate `bc.c`/`getconf.h` |
| `ports/sbase/build.sh` | Cross-build ~91 upstream sbase utilities per arch (manifest-driven kernel embed) |
| `ports/sbase/` | `.myos.patch` files, `bins.txt`, compat headers, arch soft-float shims |
| `ports/oksh/fetch.sh` | Clone pinned [oksh](https://github.com/ibara/oksh) 7.9 into `target/oksh-src` |
| `ports/oksh/prepare.sh` | Sync upstream tree; apply myos patches; install checked-in `pconfig.h` |
| `ports/oksh/build.sh` | Cross-build oksh per arch (`target/oksh-*-unknown-none`) |
| `ports/oksh/` | `pconfig.h` (`configure --no-thanks --enable-small --disable-curses`) and `.myos.patch` files |
| `c/hello.c` | Minimal newlib smoke (`c ok` via `write()`) |
| `ports/tcc/fetch.sh` | Clone pinned [TinyCC](https://github.com/TinyCC/tinycc) into `target/tcc-src` |
| `ports/tcc/prepare.sh` | Sync upstream tree; generate `tccdefs_.h`; apply `tccrun` mmap/-run glue |
| `ports/tcc/build.sh` | Cross-build native tcc per `*-unknown-myos` triple (`target/tcc-*`) |

Implemented libgloss hooks call real syscalls where they exist (`write`, `read`,
`open` (writable on tmpfs/devfs), `close`, `brk`, `fork`, `wait`/`waitpid`, `pipe`, `dup2`,
`execve`, `stat` via **`SYS_STAT` (12)**, `chdir`/`getcwd` via **`SYS_CHDIR`/`SYS_GETCWD`**,
`mkdir`/`rmdir`/`unlink`/`rename`/`symlink`/`readlink` via **`SYS_MKDIR`…`SYS_READLINK` (17–22)**
on writable mounts — today **tmpfs**). Anonymous **`mmap`/`munmap`/`mprotect`** (`SYS_MMAP` 23 … `SYS_MPROTECT` 25) and **`lseek`** (`SYS_LSEEK` 26) back TinyCC `-run`.
`opendir`/`readdir`/`closedir` use `SYS_LISTDIR`. Relative paths (including `.`)
are resolved against the per-task cwd in the kernel.
Do **not** use `-DMISSING_SYSCALL_NAMES` (libgloss exports `_write`, not `write`).

Upstream sbase (`cat`, `true`, `ls`, `pwd`, …) is fetched at build time; only
small `.myos.patch` files live in-tree (CI smoke strings, myos exec argv quirks).
Tools use upstream newlib stdio (`puts`, `printf`, `fshut`) and libutil; the
kernel enables user SIMD/FP (x86_64 SSE, AArch64 NEON) so `-O2` libc code does
not fault on stdio init.
Libgloss adds `clock_gettime` (libc already owns `time`/`localtime`), flat
`getpwuid`/`getpwnam`/`getgrgid` (root),
`fcntl(F_DUPFD)` (high fds for oksh `FDBASE`; link `fcntl.o` so it overrides
newlib's ENOSYS `fcntl`), no-op `tcgetattr`/`tcsetattr`,
`readlink` (`ENOSYS`), POSIX stubs for read-only VFS, and `sys/sysmacros.h` so
upstream `ls -l` links. The kernel mounts **sbasefs** at `/s/` with one ELF per
tool (e.g. `/s/cat`, `/s/echo`, `/s/ls` — 91 utilities today); CI on all
arches checks `sbase ok` from `/s/echo` and `sls ok` from `/s/ls` via CI-only
`/heap` (typed at `$`).

`/sh` is oksh. PATH is `/s:/c:/` (sbase, coreutils, then bootfs root). ubase
getty/login live at `/u/getty` and `/u/login` (not on PATH). Interactive CI
types `root` at `login: `, then commands at `$ `; unknown
commands print `not found`. Job control, SIGCHLD, curses, history files, and
heredoc `/tmp` are stubbed for v1 (`--enable-small`, jobs wait via blocking
`waitpid(-1)`). `user/sh` remains in-tree until CI is green.

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
| `kernel/src/fs/` | VFS mount table (`vfs.rs`) + bootfs/tmpfs/devfs/procfs backends |
| `kernel/src/console.rs` | Dual console: serial + Limine framebuffer mirror |
| `kernel/src/input.rs` | Stdin ring buffer: PS/2 keyboard + serial (fd 0) |
| `kernel/src/arch/x86/keyboard.rs` | PS/2 keyboard via 8042 (poll, US QWERTY set 1) |
| `kernel/link.ld` | Higher-half (`0xffffffff80000000`) linker script |
| `kernel/src/heap.rs` | 256 KiB `linked_list_allocator` heap from Limine usable+HHDM |
| `kernel/src/task/` | Round-robin kernel threads + user tasks: `yield_now` + timer preemption |
| `kernel/src/modules/` | ELF64 loader, `KernelApi` wrappers, loaded-module registry |
| `modules/abi` | Shared `KernelApi` / `module_init` C ABI (v7: PCI/DMA/`dev_register` after v6 VFS) |
| `modules/hello` | Sample module: embedded **and** ESP `boot/hello` via Limine |
| `modules/stubfs` | Sample prefixed mount: `vfs_mount` at `/disk`, file `/disk/ping` |
| `modules/fat` | FAT16 kernel module: `blk_read` + `vfs_register("msg")` from root `MSG` |
| `modules/ext2` | writable ext2 (rev1, 1KiB): `ModuleVfsOps`, bind at `mount(2)` |
| `modules/virtio_net` | modern virtio-pci net, poll RX/TX, `/dev/net0` Ethernet frames |
| `user/init` | PID1-style: baked in, smoke fork/`/ok`, execs `/sh` |
| `user/sh` | Legacy tiny shell (not `/sh`; kept in-tree) |
| `ports/` | Userspace ports (sbase, ubase, oksh, ripgrep, coreutils, tcc); source fetched at build |
| `ports/oksh/` | oksh pin patches + `pconfig.h` |
| `user/echo` | Print argv; installed as bootfs `/myos_echo` |
| `user/cat` | Read a bootfs file; installed as bootfs `/myos_cat` |
| `user/ls` | List bootfs entries; installed as bootfs `/myos_ls` |
| `user/lib` | Shared `myos_user` syscall/argv/`Heap` helpers |
| `user/heap` | CI-only heavy smoke (std/C/sbase/uutils/ripgrep/tcc/bigalloc); typed as `heap` at `$` |
| `user/ok` | Slim always-on boot smoke (`alloc`/`user`/`fat`/`disk`/`proc` markers); ESP `boot/ok` |
| `user/mount` | `mount` with no args prints `/proc/mounts`; `mount src tgt fstype` issues `SYS_MOUNT` |
| `targets/` | Custom Rust target specs (`x86_64-unknown-myos.json`) |
| `toolchain/newlib/` | newlib libgloss port + fetch/build scripts |
| `toolchain/std/pal/` | Rust `std` PAL skeleton + porting notes |
| `kernel/src/arch/x86/` | COM1, GDT (user segs)/TSS RSP0/IDT/xAPIC, PCI, legacy virtio-blk, isa-debug-exit |
| `kernel/src/arch/x86/pci.rs` | PCI config via `0xCF8`/`0xCFC`; find virtio-blk |
| `kernel/src/arch/aarch64/` | PL011, TTBR0 device map, GICv2 timer, lower-EL SVC, virtio-mmio blk, PSCI off |
| `kernel/src/framebuffer.rs` | Pixel writer for a Limine framebuffer |
| `kernel/src/font.rs` | Tiny 8x8 bitmap font |
| `.cargo/config.toml` | `bindeps` (artifact dependencies) |
| `rust-toolchain.toml` | pinned nightly + `llvm-tools-preview` + rust-src + targets |
| `scripts/` | Shared helpers (`myos-c-userspace-lib.sh`, rustc wrappers) + thin wrappers for old script names |
| `scripts/ci-registry.sh` | GHCR pull/push of userspace port outputs (oras), keyed by stamp hash |
| `.github/workflows/ci.yml` | PR/push CI: rust-cache is cargo-only; userspace ELFs come from GHCR |
| `.github/workflows/iso.yml` | Manual `workflow_dispatch` x86_64 hybrid ISO artifact |

## CI

GitHub Actions caches Cargo's registry/`target` with Swatinem/rust-cache
(`prefix-key: limine-8.3-6`). Userspace port **outputs** (newlib prefixes,
sbase/oksh/ubase/ripgrep/tcc/uutils ELFs, stamps) are **not** in rust-cache.
They are OCI artifacts on GHCR, one package per port, tagged with the stamp
hash from `scripts/myos-c-userspace-lib.sh` (e.g.
`ghcr.io/davide-matasci/myos/ci-sbase:<sha256>`). `skip-if-fresh`
(`myos_*_is_current`) is still the local truth after a pull. Source
checkouts (`*-src`) are never cached.

The first run after a stamp change is a miss, then a push. Later runs with
the same hashes hit. Packages should be **public** so fork PRs can pull;
the first push creates them. If they stay private, set visibility once in
GitHub → Packages → each `myos/ci-*` → Change visibility → public.

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
