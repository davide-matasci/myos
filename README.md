# myos

**A minimal, readable operating system kernel written in Rust.**

myos boots in QEMU on **x86_64**, **AArch64**, and **RISC-V** through [Limine](https://github.com/limine-bootloader/limine), and reaches a fully interactive shell with login, userspace programs, and a modular VFS — in under 33k lines of Rust and C.

This is a starting point to grow into a real OS, not a feature dump.

---

## Features

- **Multi-arch boot** — x86_64 (BIOS + UEFI), AArch64, RISC-V via Limine protocol revision 6
- **Interactive shell** — getty → login (`root`, empty password) → [oksh](https://github.com/ibara/oksh) 7.9
- **Rust kernel** — `#![no_std]`, higher-half link, HHDM memory, preemptive round-robin scheduler
- **Kernel modules** — one ELF loader; every driver and filesystem is a module (console, virtio-blk, NVMe, virtio-net, netfs, FAT16, ext2, …), and so is the Linux syscall layer; listed in `limine.conf` and loadable at runtime with `insmod`
- **VFS with multiple backends** — bootfs, tmpfs, devfs, procfs, FAT16, ext2
- **Framebuffer** — `/dev/fb/ctl` (geometry, taking the screen from the console) and `/dev/fb/data` (pixels, `mmap(MAP_SHARED)`), served by the console module (`docs/fb.md`); the keyboard's presses and releases at `/dev/console/kbd` (`docs/tty.md`)
- **Userspace ELFs** — Rust `#![no_std]` programs + Rust `std` smoke + full newlib/libgloss C toolchain
- **Ported userspace** — sbase, ubase, uutils coreutils, ripgrep, TinyCC (all fetched at build)
- **Networking** — virtio-net kernel module (RX interrupts: MSI-X on x86_64, INTx on aarch64/riscv64) + smoltcp in userspace; `/ping` works on all arches
- **Userspace BSD sockets** — libgloss shim over Plan 9 `/net` (no socket syscall); trimmed `curl` HTTPS GET; `AF_UNIX` stream sockets over `/net/unix`, kept in the kernel (`docs/sockets-unix.md`)
- **CI** — GitHub Actions with rust-cache; userspace port outputs are OCI artifacts on GHCR
- **Optional: Linux syscall compatibility** — the `linux` kernel module (loaded at boot with `--features linux_compat`, or `insmod /lib/modules/linux`) runs musl binaries (x86_64, aarch64, riscv64) via `linux PROGRAM` (see `docs/linux-compat.md`)

---

## Prerequisites

- [rustup](https://rustup.rs/) — `rust-toolchain.toml` pins **nightly-2026-07-26** and installs components automatically
- QEMU (`qemu-system-x86`, `qemu-system-arm`, `qemu-efi-aarch64`, `qemu-efi-riscv64`)
- `clang` + `lld` — cross-compiler and linker for C userspace on every arch (`ld.lld`)
- `make` — newlib build
- `git` — fetching upstream port sources
- `gh` — GitHub CLI (CI registry pulls)
- `libc6-dev` — host CRT (`Scrt1.o`) for the Limine host tool
- `rsync` — preparing oksh/sbase build trees
- `patch` — applying `.myos.patch` files
- `curl` — fetching the pinned Limine binary on first build
- `xorriso` — hybrid ISO output (`cargo run -- iso` only)

On Ubuntu:

```sh
sudo apt install qemu-system-x86 qemu-system-arm qemu-efi-aarch64 \
  clang lld make git gh libc6-dev rsync patch curl xorriso
```

On macOS (Homebrew):

```sh
brew install qemu llvm lld texinfo coreutils
# Homebrew's llvm is keg-only (no ld.lld on PATH) — the newlib/ports cross
# wrappers exec ld.lld, so prepend it:
export PATH="/opt/homebrew/opt/llvm/bin:$PATH"   # /usr/local/opt/llvm/bin on Intel
```

Notes:
- `llvm` provides `llvm-strip`/`llvm-ar` used by the cross wrappers; `lld` provides `ld.lld` (the ELF linker — on current Homebrew the `llvm` formula alone no longer includes it); `texinfo` provides `makeinfo` (newlib docs; its `missing` shim only warns, but installing keeps the build log clean); `coreutils` provides `nproc` (also made optional in `toolchain/newlib/build.sh`).
- macOS's stock `/bin/bash` is 3.2 (2007). `brew install bash` with Homebrew's bin first in `PATH` avoids bash-3.2 parser quirks in build scripts.

---

## Quick Start

```sh
cargo run
```

Builds x86_64 kernel, wraps in Limine GPT+FAT ESP (BIOS + UEFI), writes `target/fat.img`, starts QEMU. You'll see `Hello from myos`; close window to exit. The first build also cross-builds everything the image carries (every port with a `port.env` under `ports/`, `user/` and `toolchain/`, newlib included; see `docs/ports.md`), so it takes a while; a missing piece is a build error, never a silently smaller image.

```sh
cargo run -- uefi        # x86_64 UEFI
cargo run -- aarch64     # AArch64 (AAVMF)
cargo run -- riscv64     # RISC-V (Edk2)
cargo run -- iso         # write target/myos-x86_64.iso
cargo run --release      # release build
```

**Boot tests** — boot headless, log in and run the test list in the guest (`docs/testing.md`):

```sh
cargo run -- test-mini            # the quick list (every pull request runs it)
cargo run -- uefi test-mini
cargo run -- aarch64 test-full    # the full list: packages, HTTPS, SSH, os-test (needs the network)
```

**Interactive use** — type at `$` prompt (PS/2 keyboard in QEMU window on x86; serial on all arches):

```sh
cargo run          # x86 BIOS — keyboard or serial
cargo run -- uefi  # x86 UEFI
cargo run -- aarch64   # serial only for now
```

The boot test types `root` at `login: ` (the password is empty), then `sh /lib/myos-tests/run.sh mini|full` at `$ `.

---

## Architecture Overview

```
Boot (Limine)
  └─ Limine HHDM + memmap + framebuffer + modules
       └─ Kernel (higher-half, #![no_std])
            ├─ Heap (linked-list allocator, a quarter of RAM: 64 MiB to 1 GiB)
            ├─ Scheduler (round-robin kernel threads + user tasks)
            ├─ VFS (mount table → bootfs / tmpfs / devfs / procfs / ext2 / netfs)
            ├─ Modules (Limine list, in order): console, stubfs, hello, pci_enum,
            │     acpi, virtio_blk, nvme, virtio_net, netfs, fat, ext2
            └─ Userspace (ELF processes)
                 ├─ /ok smoke (always-on alloc/user/fat/disk/proc markers)
                 ├─ /netd (smoltcp over /dev/net0; only opener of net0)
                 ├─ getty → login → /sh (oksh 7.9 via newlib/libgloss)
                 └─ CI /heap: std / C / sbase / uutils / ripgrep / tcc
```

### Boot
Limine protocol base revision 6 (`limine` crate 0.6.5). Host tool fetches pinned Limine `v12.6.1`, writes GPT+FAT ESP, `limine.conf`, kernel ELF and the module ELFs (`boot/modules/<name>`, `src/limine_image.rs` `BOOT_MODULES`). On x86, `limine bios-install` makes the image BIOS+UEFI bootable. No `bootloader` crate, no QEMU `-kernel`, no Multiboot.

### Memory
Kernel linked in higher half (`0xffffffff80000000` on x86_64). Limine provides HHDM; usable memory = `phys + HHDM`. Page tables allocated from bump allocator after heap. AArch64 device block (UART, GIC, virtio-mmio) identity-mapped via `TTBR0`.

### Scheduling
Round-robin kernel threads + user tasks across all online CPUs; a user process can run several threads, which share its address space and run on its home CPU (`docs/threads.md`) (Limine MP bring-up on x86_64, AArch64 and RISC-V; see `docs/pci-acpi-smp.md`). `task::yield_now()` cooperative; timer IRQ calls `task::schedule()` after EOI → preemptive even in user mode. Blocking waits (`read` on a tty/pipe/pty, `wait`, `nanosleep`, `select`/`poll`) put the task in a `Blocked` state and are woken by the producer (`task::wake`), a deadline or a signal; idle CPUs halt (`hlt`/`wfi`) until an interrupt or a targeted reschedule IPI. `/proc/cpuinfo` shows per-CPU schedule and idle-halt counts. x86_64: xAPIC timer at 1 kHz, TSC calibrated against the PIT for the monotonic clock. AArch64: GICv2 generic timer, `CNTVCT`. RISC-V: `stimecmp`, `time` CSR at the device tree's `timebase-frequency`. AArch64 and RISC-V tick at 100 Hz but program the timer for the earliest sleep deadline when it is sooner, so `nanosleep` / `poll` timeouts are not rounded up to 10 ms.

### Console & Input
Dual console: serial (kernel) + Limine framebuffer (the `console` module; boot output before it loads is replayed to it). Stdin (fd 0) merges the module's keyboard (PS/2 on x86 via 8042 probe, virtio-input on the `virt` boards) and serial simultaneously.

---

## Repository Layout

| Path | Role |
|------|------|
| `src/main.rs` | Host launcher: QEMU (BIOS/UEFI/AArch64/RISC-V) + second virtio-blk disk |
| `src/limine_image.rs` | GPT+FAT ESP writer + Limine fetch + `limine.conf` + `fat.img` |
| `build.rs` | Fetch Limine; wrap x86_64 kernel in BIOS+UEFI images; write `fat.img` |
| `kernel/src/main.rs` | `#![no_std]` Limine entry: heap, IRQs, scheduler, bootfs, Limine modules, user init |
| `kernel/src/limine_boot.rs` | Limine requests (HHDM, memmap, DTB, FB, modules, executable addr) |
| `kernel/src/dt.rs` | Device tree (aarch64, riscv64): device bases, PCI INTx `interrupt-map`, `virtio,mmio` nodes, `timebase-frequency` (`fdt` crate) |
| `kernel/src/mm.rs` | Physical frame allocator (after 256 KiB heap; page tables, user pages, virtqueues) |
| `kernel/src/blk.rs` | Block-device registry filled by driver modules (`blk_register`); `/dev/<name>` + sector/byte I/O |
| `kernel/src/arch/` | All per-arch code: boot, UART, interrupts, PCI, user entry/paging (`user`, `upaging`), context switch, FPU, clock, SMP glue |
| `kernel/src/console.rs` | Serial console + the `console` module's screen/keyboard hooks (early-output replay) |
| `kernel/src/input.rs` | Stdin line discipline: module keyboard + serial → fd 0 |
| `kernel/src/heap.rs` | `linked_list_allocator` heap sized from memory (also holds tmpfs data) |
| `kernel/src/task/` | Scheduler records (`Task`) + per-process blocks (`process.rs`): yield, preemption, fork/exec/wait |
| `kernel/src/fs/` | VFS + bootfs/tmpfs/devfs/procfs backends |
| `kernel/src/modules/` | ELF64 loader, KernelApi wrappers, loaded-module registry |
| `modules/abi` | Shared `#[repr(C)]` KernelApi (v14: PCI/DMA/`dev_register`/`blk_register`/`console_register`/`personality_register`/`dt_mmio_find`) |
| `modules/virtq` | Split virtqueue helpers shared by the virtio modules |
| `modules/console` | Framebuffer text screen and `/dev/fb`, PS/2 + virtio-input keyboards, loadable keymap (`keymaps/`; scancode decoding in the host-testable `ps2-scancode` crate) |
| `modules/virtio_blk` | virtio-blk `/dev/vd*`: PCI legacy I/O (x86_64) or virtio-mmio (aarch64, riscv64) |
| `modules/nvme` | NVMe `/dev/nvmeXn1` (PCI class 01/08, polled queues) |
| `modules/hello` | Sample module (`[ OK ] hello`) |
| `modules/stubfs` | Sample prefixed mount via `vfs_mount` at `/disk` |
| `modules/fat` | FAT16 kernel module: `blk_read` + `vfs_register("msg")` |
| `modules/ext2` | Writable ext2: `ModuleVfsOps` over the `ext2fs` crate (`modules/ext2/ext2fs`, also `mkfs.ext2`'s), host-tested against e2fsprogs |
| `modules/virtio_net` | Modern virtio-pci net: poll RX/TX, `/dev/net0` Ethernet frames |
| `modules/netfs` | Plan 9 `/net` + `/dev/netd` channel to userspace netd; `/net/unix` local connections |
| `modules/linux` | Linux syscall compatibility layer: a syscall *personality* (`personality_register`) for musl binaries |
| `user/init` | PID1: smoke fork/`/ok`, fork `/netd`, exec `/sh` (baked in) |
| `user/sh` | Legacy tiny shell (not `/sh`; kept in-tree) |
| `user/ok` | Slim always-on boot smoke (alloc/user/fat/disk/proc) |
| `user/heap` | CI-only heavy smoke (std/C/sbase/uutils/ripgrep/tcc/bigalloc) |
| `user/netd` | Userspace smoltcp over `/dev/net0` |
| `user/insmod` | `insmod /lib/modules/<name>`: load a kernel module at runtime (`SYS_INSMOD`) |
| `user/rmmod` | `rmmod <name>`: unload a kernel module that provides nothing any more (`SYS_RMMOD`) |
| `user/lib` | Shared `myos_user` syscall wrappers, argv parser, `Heap` allocator |
| `user/c` | Native C programs (newlib): `hello` and the boot-CI smokes installed as `/bin/etc/*` |
| `user/echo/cat/ls` | Bootfs demos (`/myos_echo`, `/myos_cat`, `/myos_ls`) |
| `user/std` | The Rust `std` demo programs (`/bin/std/{hello,cat,echo,bigalloc}`) |
| `user/get-myos` | `get-myos`: installs packages (ports the image does not carry) from a mirror, `docs/packages.md` |
| `user/mount` | `mount` prints `/proc/mounts` or issues `SYS_MOUNT` (`mount SRC TARGET FSTYPE`, `bind` for a bind mount) |
| `ports/` | Userspace ports in the image: source fetched at build (sbase, ubase, oksh, ripgrep, coreutils, tcc, curl, dropbear, ...), one `port.env` descriptor each (`docs/ports.md`) |
| `packages/` | Ports CI builds and publishes but the image does not carry (vim, git, lynx, lua, make, os-test; `get-myos NAME` installs them, `docs/packages.md`); moving a directory here (or back to `ports/`) is the whole change |
| `toolchain/newlib/` | newlib 4.4.0 + libgloss/myos syscall adapters |
| `toolchain/std/` | Rust `std` PAL skeleton, sysroot build scripts (the `sysroot` port) |
| `targets/` | Custom Rust target specs (`x86_64-unknown-myos`, `aarch64-unknown-myos`, `riscv64imac-unknown-myos`) |
| `scripts/` | Thin wrappers for port builds; `ports.sh` (the descriptors); CI registry (`myos-c-userspace-lib.sh`) |
| `linux-compat/` | Userspace of the Linux layer: the `linux` launcher (every image), musl tests and `get-alpine` (feature `linux_compat`) |

---

## Build & Run Detail

```sh
cargo build
```

Produces:
- `target/bios.img` — BIOS+UEFI hybrid disk
- `target/uefi.img` — UEFI-only ESP
- `target/fat.img` — 20 MiB FAT16 data disk (second virtio-blk)
- `target/aarch64.img` — AArch64 ESP
- `target/riscv64.img` — RISC-V ESP

### x86_64 BIOS
```sh
qemu-system-x86_64 -m 256 \
  -drive format=raw,file=target/bios.img \
  -drive if=none,id=vd0,format=raw,file=target/fat.img \
  -device virtio-blk-pci,drive=vd0,disable-modern=on \
  -serial stdio
```

### x86_64 UEFI
Same with `target/uefi.img`.

### AArch64
```sh
qemu-system-aarch64 -m 256 -cpu cortex-a72 -machine virt,gic-version=2 \
  -drive format=raw,file=target/aarch64.img \
  -drive if=none,id=vd0,format=raw,file=target/fat.img \
  -device virtio-blk-device,drive=vd0 \
  -bios /usr/share/AAVMF/AAVMF_CODE.fd -serial stdio
```

### RISC-V
```sh
qemu-system-riscv64 -m 256 -machine virt \
  -drive format=raw,file=target/riscv64.img \
  -bios /usr/share/qemu/ovmf-bin/Edk2-riscv64.fd -serial stdio
```

### ISO (hybrid)
```sh
cargo run -- iso          # writes target/myos-x86_64.iso
qemu-system-x86_64 -m 256 -cdrom target/myos-x86_64.iso -serial stdio
```

### Second disk (`target/fat.img`)
Attached as second virtio-blk in all QEMU runs. Holds `/msg` for `[ OK ] fat` / msg markers. Bare metal without it still boots to shell.

---

## Real Hardware

Write the Limine disk image to USB/internal drive (`target/bios.img` for BIOS, `target/uefi.img` for UEFI). Framebuffer mirrors serial — boot progress scrolls on screen.

**stdin** merges keyboard and serial. Keyboard characters come from a **loadable keymap** (default Swiss German via `user/init` → `/lib/kbd/ch.map`, US fallback; see `docs/keymap.md`). Serial always available and does not need a map.

| Arch | Serial port | Baud rate |
|------|-------------|-----------|
| x86_64 | COM1 (I/O `0x3F8`) | 38400 8N1 |
| AArch64 | PL011 UART | 115200 8N1 |
| RISC-V | UART | 115200 8N1 |

---

## VFS & Filesystems (Summary)

- **VFS** — mount table with longest-prefix routing and bind mounts (`mount SRC TARGET bind`: a directory or file seen at a second place too, the target need not exist; `get-myos` installs packages this way); `vfs::mounts_text()` exports `/proc/mounts`
- **bootfs** — read-only embedded namespace at `/`; Limine ESP modules override; demos use `myos_` prefix
- **procfs** — `/proc/mounts` (generated, not stored bytes)
- **tmpfs/devfs** — writable mount for `O_CREAT`; device nodes
- **virtio-blk / NVMe** — modules registering `/dev/vda`… and `/dev/nvme0n1` through `blk_register`; loaded before the filesystem modules
- **FAT16 module** — parses BPB, walks cluster chain, registers `/msg` from root `MSG`
- **ext2 module** — the ext2 Linux and e2fsprogs know (1/2/4 KiB blocks, block groups, indirect blocks up to triple, symlinks, rename, sparse superblocks, files over 2 GiB), bound via `mount(2)` fstype `ext2` on a disk `mkfs.ext2` (or Linux's `mke2fs -t ext2`) formatted; `cargo test -p ext2fs` checks it against `e2fsck` and `debugfs`. CI boots carry an empty 4 GiB scratch disk (`/dev/nvme1n1`) for big filesystems
- **virtio-net / netfs / netd** — kernel virtio-net → `/dev/net0` Ethernet; netfs mounts Plan 9 `/net`; netd runs smoltcp in userspace over `/dev/netd`; `/ping <ipv4>` uses `/net/icmp`

---

## Kernel Modules

Kernel modules are ELFs in RAM. One loader copies `PT_LOAD`, applies relocs, calls `module_init`. The kernel embeds none of them:

| | Boot (Limine) | Runtime (`insmod`) |
|---|---|---|
| Bytes live in | `boot/modules/<name>` on the ESP, listed in `limine.conf` (`module_path`, load order) | `/lib/modules/<name>` in the initramfs (or any file) |
| Loaded by | `modules::load_limine_modules` right after bootfs | `SYS_INSMOD` from `/bin/custom/insmod` |

`/proc/modules` lists what is loaded. `rmmod <name>` (`SYS_RMMOD`) unloads a module that provides nothing any more: the kernel counts what each module registered through the `KernelApi` (devices, filesystems, mounts, `/proc` nodes, interrupts, the console, a personality) and refuses to unload one with a registration left, since nothing unregisters yet; `hello` unloads, a driver does not. Writing `rescan` to `/proc/pci` re-enumerates the bus and then calls every module's `module_rescan`: the block drivers bring up the controllers and disks that appeared since boot (`/dev/nvme1n1`, `/dev/vdb`, ...) and leave the known ones alone. The console module goes first (it paints the buffered boot output), then stubfs, hello, pci_enum, acpi, the block drivers (virtio_blk, nvme), virtio_net, netfs and the filesystems (fat, ext2). Modules behind a Cargo feature (`OPTIONAL_MODULES`: `linux` with `linux_compat`) are always shipped under `/lib/modules` but only listed in `limine.conf` when the feature is on.

Module exports:
```rust
unsafe extern "C" fn module_init(api: *const KernelApi) -> i32
unsafe extern "C" fn module_exit()   // optional: run by rmmod
unsafe extern "C" fn module_rescan() // optional: probe for new devices after a /proc/pci rescan
```

`KernelApi` (`modules/abi`) is a `#[repr(C)]` table, ABI v18 (append-only). Kernel fills it and passes it to `module_init`. Drivers register what they provide: `blk_register` (block devices), `dev_register` (char devices), `fs_register` / `vfs_mount` (filesystems; a backend's optional `mmap` hook maps device memory, `docs/fb.md`, and its `poll` hook reports readiness for `poll`), `console_register` (screen + keyboard), `personality_register` (a foreign syscall ABI, see `docs/linux-compat.md`); `dt_mmio_find` gives a driver its memory-mapped devices from the device tree.

### Adding a module
1. Copy `modules/hello` → `modules/foo` (keep panic=abort, opt-level=s, myos-abi, link flags)
2. Add it to the module list in `kernel/build.rs` (builds `target/foo-<triple>` for every arch) and to `BOOT_MODULES` in `src/limine_image.rs` at the position it must load (that also ships it in the initramfs and generates the `module_path` line); list the ELFs in `scripts/ci-build-kernels.sh` / `ci-pack-build-artifacts.sh`
3. Or skip the boot list and load it on demand: `insmod /lib/modules/foo`

---

## Userspace (Summary)

### Syscalls (append-only)
`write`, `exit`, `open`, `read` (fd 0 = keyboard+serial), `close`, `exec`, `fork`, `wait`, `listdir`, `brk`, `pipe`, `dup2`, `stat`, `execname`, `dupfd`, `chdir`, `getcwd`, `mkdir`, `rmdir`, `unlink`, `rename`, `symlink`, `readlink`, `mmap`, `munmap`, `mprotect`, `lseek`, `poll`, …, and threads: `thread_spawn`, `thread_exit`, `wait_addr`, `wake_addr`, `gettid` (see `docs/threads.md`).

### Init & Shell
`user/init` = PID1: baked in, smoke-tests fork/`/ok`, forks `/netd`, forks `/u/getty` and `wait()`/respawns. Getty prompts `login: ` → execs `/u/login` → accepts `root`/empty → execs `/sh`. `/sh` = oksh 7.9 with PATH `/bin/sbase:/bin/coreutils:/bin/ubase:/bin/custom:/bin/tcc:/bin/std:/bin/etc`. Editor: `vim` → `/bin/custom/vim` (FEAT_TINY; see `packages/vim/README.md`) and VCS: `git` → `/bin/custom/git` (Phase-1 local porcelain; see `packages/git/README.md`) are packages, `get-myos vim git` installs them. Framebuffer CSI includes scroll regions; `TERMCAP=/lib/termcap` (`ports/termcap`) + termios raw mode for full-screen TUI. A terminal is a directory, `data` and `ctl` (its termios and window size as text), the console at `/dev/console/`, the ptys at `/dev/pts/N/` from `/dev/pts/clone`, with `/proc/self/fd/N` and `/proc/self/tty` naming them (`docs/tty.md`).

### Rust Userspace
Syscall 9 (`brk`) backs per-process heap. `user/lib` exposes `brk`, `heap_init`, bump `GlobalAlloc`. `user/ok` smoke-tests every boot. `user/heap` = CI-only heavy suite. `std` programs link prebuilt sysroot (`toolchain/std/build-sysroot.sh`).

### C Userspace (newlib + libgloss)
Links against newlib with myos libgloss (syscall adapters + ENOSYS stubs). No new kernel syscalls needed.

```sh
./toolchain/newlib/build.sh         # fetch newlib 4.4.0, build libc + libgloss/myos
./scripts/build-c-hello.sh          # minimal write() smoke
./ports/sbase/build.sh              # ~91 sbase utilities under /s/
./ports/ubase/build.sh              # getty + login under /u/
./ports/oksh/build.sh               # oksh 7.9 as /sh
./packages/vim/build.sh             # vim FEAT_TINY (a package: get-myos vim)
./ports/zlib/build.sh               # static libz.a for git and get-myos
./packages/git/build.sh             # git Phase-1 local (a package: get-myos git)
./ports/tcc/build.sh                # TinyCC as /t/tcc (-run support)
./ports/ripgrep/build.sh            # ripgrep + PCRE2 as /c/rg
```

Implemented libgloss hooks call real syscalls where they exist; stubs return ENOSYS/EROFS for rest.

---

## CI

GitHub Actions caches Cargo with Swatinem/rust-cache (`prefix-key: limine-8.3-6`). Userspace port outputs are OCI artifacts on GHCR, one package per port (its `PORT_OUTPUTS`, `scripts/ports.sh`), tagged with stamp hash from `scripts/myos-c-userspace-lib.sh`; the ports matrices come from the descriptors. First run after stamp change = miss + push; later runs with same hashes = hit. Packages should be **public** for fork PRs. Source checkouts (`*-src`) are never cached.

Every run boots the four disk images with the quick test list (`test-mini`); the daily run and a dispatch with `full_boot` run the full list, which also installs the build's own packages into the guest with `get-myos` from a mirror the launcher serves. On master, the x86_64 hybrid ISO with the Linux layer in it (`--features linux_compat`) and the packages go to the rolling `rolling` release, get-myos's default mirror (`docs/packages.md`, `docs/testing.md`).

---

## Notes

- x86_64: CPU halts with `hlt` (QEMU stays open; the boot test attaches `isa-debug-exit` and kills QEMU when done)
- AArch64: kernel issues PSCI `SYSTEM_OFF` (QEMU treats as shutdown)
- Kernel linked in higher half; Limine sets stack, enables MMU, provides HHDM
- AArch64 device MMIO identity-mapped on `TTBR0` (not in HHDM at Limine base rev 3+)
- Modules run from HHDM heap (rwx); loader flushes I-cache on AArch64 after copy
- Limine binaries downloaded from GitHub release `v12.6.1` (sha256-pinned) into `target/limine-v12.6.1`
- `user/ok` exits early if `/msg` absent (FAT disk not present); CI fails if `[ OK ] msg` / fat markers are missing

## License

myos's own code is dual-licensed under [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.

A built image also contains third-party programs under their own licenses (some copyleft: Git, Lynx, GNU Make, TinyCC). See [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) for the full list, the source offer and the license texts in [`licenses/`](licenses/); the same files are copied into the root of the ISO built by `cargo run -- iso`.
