# myos

**A minimal, readable operating system kernel written in Rust.**

myos boots in QEMU on **x86_64**, **AArch64**, and **RISC-V** through [Limine](https://github.com/limine-bootloader/limine), and reaches a fully interactive shell with login, userspace programs, and a modular VFS — in under 33k lines of Rust and C.

This is a starting point to grow into a real OS, not a feature dump.

---

## Features

- **Multi-arch boot** — x86_64 (BIOS + UEFI), AArch64, RISC-V via Limine protocol revision 6
- **Interactive shell** — getty → login (`root`, empty password) → [oksh](https://github.com/ibara/oksh) 7.9
- **Security** — no superuser: every process runs for a user in a domain, every file has a label its path gives it, and `/etc/policy` says what each domain may do to each label (deny by default, SELinux-like); per-process namespaces narrow what a program can name, Plan 9 style (`sec ns`); passwords are checked by the kernel (`docs/security.md`)
- **Rust kernel** — `#![no_std]`, higher-half link, HHDM memory, preemptive round-robin scheduler
- **Kernel modules** — one ELF loader; every driver and filesystem is a module (console, virtio-blk, NVMe, xHCI USB with hubs and sticks, virtio-net, netfs, FAT16, ext2, …), and so is the Linux syscall layer; listed in `limine.conf` and loadable at runtime with `insmod`
- **VFS with multiple backends** — rootfs, tmpfs, devfs, procfs, FAT16, ext2
- **Framebuffer** — `/dev/fb/ctl` (geometry, taking the screen from the console) and `/dev/fb/data` (pixels, `mmap(MAP_SHARED)`), served by the console module (`docs/fb.md`); the keyboard's presses and releases at `/dev/console/kbd` (`docs/tty.md`); an X server on both, TinyX's `Xfbdev` (`get-myos tinyx`, `packages/tinyx/README.md`), with antialiased TrueType text through Xft (`packages/x11-xft/README.md`) the dwm window manager, the st terminal and the dmenu menu dwm starts (`get-myos dwm st dmenu` brings the Xft stack and the fonts with them, then `startx`; `packages/dwm/README.md`, `packages/st/README.md`, `packages/dmenu/README.md`)
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
            ├─ VFS (mount table → rootfs / tmpfs / devfs / procfs / ext2 / netfs)
            ├─ Modules (Limine list, in order): console, hello, pci_enum,
            │     acpi, virtio_blk, nvme, xhci, usb_hub, usb_storage, virtio_net, netfs, fat, ext2
            └─ Userspace (ELF processes)
                 ├─ /ok smoke (always-on alloc/user/fat/proc markers)
                 ├─ /netd (smoltcp over /dev/net0/data; only opener of net0)
                 ├─ getty → login → /sh (oksh 7.9 via newlib/libgloss)
                 └─ CI /heap: std / C / sbase / uutils / ripgrep / tcc
```

### Boot
Limine protocol base revision 6 (`limine` crate 0.6.5). Host tool fetches pinned Limine `v12.6.1`, writes GPT+FAT ESP, `limine.conf`, kernel ELF and the module ELFs (`boot/modules/<name>`, `src/limine_image.rs` `BOOT_MODULES`). On x86, `limine bios-install` makes the image BIOS+UEFI bootable. No `bootloader` crate, no QEMU `-kernel`, no Multiboot.

### Memory
Kernel linked in higher half (`0xffffffff80000000` on x86_64). Limine provides HHDM; usable memory = `phys + HHDM`. Page tables allocated from bump allocator after heap. AArch64 device block (UART, GIC, virtio-mmio) identity-mapped via `TTBR0`.

### Scheduling
Round-robin kernel threads + user tasks across all online CPUs; a user process can run several threads, which share its address space and run on its home CPU (`docs/threads.md`) (Limine MP bring-up on x86_64, AArch64 and RISC-V; see `docs/pci-acpi-smp.md`). `task::yield_now()` cooperative; timer IRQ calls `task::schedule()` after EOI → preemptive even in user mode. Blocking waits (`read` on a tty/pipe/pty, `wait`, `nanosleep`, `select`/`poll`) put the task in a `Blocked` state and are woken by the producer (`task::wake`), a deadline or a signal; idle CPUs halt (`hlt`/`wfi`) until an interrupt or a targeted reschedule IPI. `/proc/cpuinfo` shows per-CPU schedule and idle-halt counts. x86_64: LAPIC timer at 1 kHz (x2APIC when the CPU has one), TSC for the monotonic clock at the rate CPUID states or calibrated against the PIT. AArch64: generic timer (GICv2 or GICv3 PPIs), `CNTVCT`. RISC-V: `stimecmp`, `time` CSR at the device tree's `timebase-frequency`. AArch64 and RISC-V tick at 100 Hz but program the timer for the earliest sleep deadline when it is sooner, so `nanosleep` / `poll` timeouts are not rounded up to 10 ms.

### Console & Input
Dual console: serial (kernel) + Limine framebuffer (the `console` module; boot output before it loads is replayed to it). Stdin (fd 0) merges the module's keyboard (PS/2 on x86 via 8042 probe, virtio-input on the `virt` boards) and serial simultaneously.

---

## Repository Layout

| Path | Role |
|------|------|
| `src/main.rs` | Host launcher: QEMU (BIOS/UEFI/AArch64/RISC-V) + second virtio-blk disk |
| `src/limine_image.rs` | GPT+FAT ESP writer + Limine fetch + `limine.conf` + `fat.img` |
| `build.rs` | Fetch Limine; wrap x86_64 kernel in BIOS+UEFI images; write `fat.img` |
| `kernel/src/main.rs` | `#![no_std]` Limine entry: heap, IRQs, scheduler, rootfs, Limine modules, user init |
| `kernel/src/limine_boot.rs` | Limine requests (HHDM, memmap, DTB, FB, modules, executable addr) |
| `kernel/src/platform.rs` | The board description, filled once at boot from the ACPI static tables (`acpi.rs`: MADT, MCFG, SPCR, GTDT) and the device tree (`dt.rs`, `fdt` crate), shown by `/proc/platform` (`docs/pci-acpi-smp.md`) |
| `kernel/src/dt.rs` | Device tree (aarch64, riscv64): fills the platform description; PCI INTx `interrupt-map`, `virtio,mmio` nodes |
| `kernel/src/mm.rs` | Physical frame allocator (after 256 KiB heap; page tables, user pages, virtqueues) |
| `kernel/src/sec/` | The security policy (`docs/security.md`): parser, labels, domains, users, the checks the syscalls make, the passwords' SHA-256 |
| `etc/policy` | The default policy: packed as `/etc/policy`, and the kernel's fallback copy |
| `kernel/src/blk.rs` | Block-device registry filled by driver modules (`blk_register`); `/dev/<name>` + sector/byte I/O, through the block cache (`blk/cache.rs`) |
| `kernel/src/arch/` | All per-arch code: boot, UART, interrupts, PCI, user entry/paging (`user`, `upaging`), context switch, FPU, clock, SMP glue |
| `kernel/src/console.rs` | Serial console + the `console` module's screen/keyboard hooks (early-output replay) |
| `kernel/src/input.rs` | Stdin line discipline: module keyboard + serial → fd 0 |
| `kernel/src/heap.rs` | `linked_list_allocator` heap sized from memory (also holds tmpfs data) |
| `kernel/src/task/` | Scheduler records (`Task`) + per-process blocks (`process.rs`): yield, preemption, fork/exec/wait |
| `kernel/src/fs/` | VFS + rootfs/tmpfs/devfs/procfs backends |
| `kernel/src/modules/` | ELF64 loader, KernelApi wrappers, loaded-module registry |
| `modules/abi` | Shared `#[repr(C)]` KernelApi (v14: PCI/DMA/`dev_register`/`blk_register`/`console_register`/`personality_register`/`dt_mmio_find`) |
| `modules/virtq` | Split virtqueue helpers shared by the virtio modules |
| `modules/console` | Framebuffer text screen and `/dev/fb`, PS/2 + virtio-input keyboards, loadable keymap (`keymaps/`; scancode decoding in the host-testable `ps2-scancode` crate) |
| `modules/virtio_blk` | virtio-blk `/dev/vd*`: PCI legacy I/O (x86_64) or virtio-mmio (aarch64, riscv64) |
| `modules/nvme` | NVMe `/dev/nvmeXn1` (PCI class 01/08, polled queues) |
| `modules/xhci` | xHCI USB host controller: the USB bus service (`UsbHostOps`), enumeration and hot-plug on its own kernel thread, `/proc/usb` (`docs/usb.md`) |
| `modules/usb_hub` | USB hub class driver: ports, resets, the devices behind a hub |
| `modules/usb_storage` | USB mass storage (bulk-only, SCSI): `/dev/sdX`, gone with the stick |
| `modules/hello` | Sample module (`[ OK ] hello`) |
| `modules/fat` | FAT16 kernel module: `blk_read` + `vfs_register("msg")` |
| `modules/ext2` | Writable ext2: `ModuleVfsOps` over the `ext2fs` crate (`modules/ext2/ext2fs`, also `mkfs.ext2`'s), host-tested against e2fsprogs |
| `modules/virtio_net` | Modern virtio-pci net: `/dev/net0/` (`data` Ethernet frames, `ctl` the MAC and interrupt), RX interrupt wakes `poll` |
| `modules/netfs` | Plan 9 `/net` + `/dev/netd/data` channel to userspace netd; `/net/unix` local connections |
| `modules/linux` | Linux syscall compatibility layer: a syscall *personality* (`personality_register`) for musl binaries |
| `user/init` | PID1: smoke fork/`/ok`, fork `/netd`, exec `/sh` (baked in) |
| `user/sh` | Legacy tiny shell (not `/sh`; kept in-tree) |
| `user/ok` | Slim always-on boot smoke (alloc/user/fat/proc) |
| `user/heap` | CI-only heavy smoke (std/C/sbase/uutils/ripgrep/tcc/bigalloc) |
| `user/netd` | Userspace smoltcp over `/dev/net0/data` |
| `user/insmod` | `insmod /lib/modules/<name>`: load a kernel module at runtime (`SYS_INSMOD`) |
| `user/rmmod` | `rmmod <name>`: unload a kernel module that provides nothing any more (`SYS_RMMOD`) |
| `user/lib` | Shared `myos_user` syscall wrappers, argv parser, `Heap` allocator |
| `user/c` | Native C programs (newlib): `hello` and the boot-CI smokes installed as `/bin/etc/*` |
| `user/echo/cat/ls` | Bootfs demos (`/myos_echo`, `/myos_cat`, `/myos_ls`) |
| `user/std` | The Rust `std` demo programs (`/bin/std/{hello,cat,echo,bigalloc}`) |
| `user/get-myos` | `get-myos`: installs packages (ports the image does not carry) from a mirror, `docs/packages.md` |
| `user/mount` | `mount` prints `/proc/mounts` or issues `SYS_MOUNT` (`mount SRC TARGET FSTYPE`, `bind` for a bind mount); says why a mount failed |
| `user/umount` | `umount DIR` detaches the disk mounted there (`SYS_UMOUNT`) |
| `ports/` | Userspace ports in the image: source fetched at build (sbase, ubase, oksh, ripgrep, coreutils, tcc, curl, dropbear, ...), one `port.env` descriptor each (`docs/ports.md`) |
| `packages/` | Ports CI builds and publishes but the image does not carry (vim, git, lynx, lua, make, os-test, x11-libs, tinyx, x11-xft, x11-fonts, dwm, st, dmenu; `get-myos NAME` installs them, `docs/packages.md`); moving a directory here (or back to `ports/`) is the whole change |
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
The GIC version must match the device tree the launcher packed into the
image (`MYOS_AARCH64_GIC=3 cargo run -- aarch64` for a GICv3 board).

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
- **Open files** — an fd, a file mapping or a running program holds its file, not its name (`kernel/src/fs/node.rs`): it follows a rename of the file or of a directory above it, and a file unlinked or replaced (`rename` over it) while held stays readable and writable through it until the last holder lets go (tmpfs keeps it under a name no path reaches, a module filesystem with the `unlink_keep` hook its inode, ext2 does; on the others the holder's reads and writes fail instead). A filesystem that hands out file ids (tmpfs's files, a module's `file_id` hook: ext2's inode numbers) is read, written, sized and timed through the id once a file is open, a lookup by path only when it is opened; the node table shares one node per file, and `stat`'s `st_ino` is the id. Renames and unlinks exclude the calls that use a path or an open file, so none of them reaches a new file that took an old name
- **File locks** (`kernel/src/fs/lock.rs`) — advisory, as on Unix: `flock` locks belong to the open file description (a `dup` or a `fork` shares them) and go with its last fd; `fcntl` record locks (`F_SETLK`, `F_SETLKW`, `F_GETLK`, libc's `lockf`) lock byte ranges for the process and go when it exits or closes any fd on the file (POSIX); Linux's open-file-description record locks are the description's. The two kinds never meet. A request that conflicts fails with `EWOULDBLOCK` without waiting, or waits until the holder lets go, a caught signal ending the wait (`EINTR`). No hard links, on purpose: a file has one name (`docs/security.md`), `link` fails with `EPERM`
- **Mounting a disk** — `mount /dev/sda /mnt ext2`: the target is any existing directory that is not a mount point yet, at the top level (`/mnt`, an empty directory of the image) or anywhere below (`mkdir /tmp/usb`); a disk is mounted once. `umount DIR` gives the directory back once no file on the disk is open or mapped and nothing is mounted below it, after the filesystem's `unmount` hook wrote back what it caches (ext2). A directory a mount hangs from can be neither renamed nor removed. The kernel's own trees (`/`, `/tmp`, `/dev`, `/proc`) cannot be unmounted
- **rootfs** — the read-only tree at `/`: every file of the image in one sorted table (the programs the kernel embeds, the Limine modules, the initramfs, which overrides an embedded program; a module's `vfs_register`); a directory is implied by its files (`mnt/.keep` for an empty one). `/tmp`, `/dev`, `/proc` and module filesystems are mounted over it
- **Security** — every path the syscalls touch is checked against the policy (`kernel/src/sec`, `/etc/policy` from `etc/policy`) and the caller's namespace (`kernel/src/task/ns.rs`, which also implements `chroot`); labels come from the canonical path (bind mounts resolved), so a second name gives no second label (`docs/security.md`)
- **procfs** — `/proc/mounts` (generated, not stored bytes); `/proc/self/ctx` (the caller's uid, user and domain) and `/proc/sys/security/users` (the policy's users, what libgloss's `getpwnam` and friends read); `/proc/sys/kernel/hostname` holds the host name ("myos" at boot; `echo name > /proc/sys/kernel/hostname`, or `hostname name`, sets it, up to 64 bytes)
- **rootfs** — the read-only tree at `/`: every file of the image in one sorted table: the initramfs, the cpio archive Limine loads next to the kernel modules, unpacked by reference (no copy), and a module's `vfs_register`. The kernel embeds no program: it starts `/bin/custom/init` from the initramfs; a directory is implied by its files (`mnt/.keep` for an empty one). `/tmp`, `/dev`, `/proc` and module filesystems are mounted over it
- **procfs** — `/proc/mounts` (generated, not stored bytes); `/proc/sys/kernel/hostname` holds the host name ("myos" at boot; `echo name > /proc/sys/kernel/hostname`, or `hostname name`, sets it, up to 64 bytes)
- **tmpfs/devfs** — writable mount for `O_CREAT`; device nodes
- **virtio-blk / NVMe** — modules registering `/dev/vda`… and `/dev/nvme0n1` through `blk_register`; loaded before the filesystem modules
- **Block cache** — what is read from a disk is kept in 4 KiB pages, up to an eighth of RAM (least recently used out first, all of it given back when memory runs out; `BlockCacheKiB` in `/proc/meminfo`); writes go to the disk and update it
- **FAT16 module** — parses BPB, walks cluster chain, registers `/msg` from root `MSG`
- **ext2 module** — the ext2 Linux and e2fsprogs know (1/2/4 KiB blocks, block groups, indirect blocks up to triple, symlinks, rename, sparse superblocks, files over 2 GiB), bound via `mount(2)` fstype `ext2` on a disk `mkfs.ext2` (or Linux's `mke2fs -t ext2`) formatted; `cargo test -p ext2fs` checks it against `e2fsck` and `debugfs`. CI boots carry an empty 4 GiB scratch disk (`/dev/nvme1n1`) for big filesystems
- **virtio-net / netfs / netd** — kernel virtio-net → `/dev/net0/data` Ethernet; netfs mounts Plan 9 `/net`; netd runs smoltcp in userspace over `/dev/netd/data`; `/ping <ipv4>` uses `/net/icmp`

---

## Kernel Modules

Kernel modules are ELFs in RAM. One loader copies `PT_LOAD`, applies relocs, calls `module_init`. The kernel embeds none of them:

| | Boot (Limine) | Runtime (`insmod`) |
|---|---|---|
| Bytes live in | `boot/modules/<name>` on the ESP, listed in `limine.conf` (`module_path`, load order) | `/lib/modules/<name>` in the initramfs (or any file) |
| Loaded by | `modules::load_limine_modules` right after rootfs | `SYS_INSMOD` from `/bin/custom/insmod` |

`/proc/modules` lists what is loaded. `rmmod <name>` (`SYS_RMMOD`) unloads a module that provides nothing any more: the kernel counts what each module registered through the `KernelApi` (devices, filesystems, mounts, `/proc` nodes, interrupts, the console, a personality, a service, a thread, a service it looked up) and refuses to unload one with a registration left; only block devices unregister (`blk_unregister`, a USB stick pulled out); `hello` unloads, a driver does not. Writing `rescan` to `/proc/pci` re-enumerates the bus and then calls every module's `module_rescan`: the block drivers bring up the controllers and disks that appeared since boot (`/dev/nvme1n1`, `/dev/vdb`, ...) and leave the known ones alone. The console module goes first (it paints the buffered boot output), then hello, pci_enum, acpi, the block drivers (virtio_blk, nvme), the USB bus (xhci, then its class drivers usb_hub and usb_storage), virtio_net, netfs and the filesystems (fat, ext2). Modules behind a Cargo feature (`OPTIONAL_MODULES`: `linux` with `linux_compat`) are always shipped under `/lib/modules` but only listed in `limine.conf` when the feature is on.

Module exports:
```rust
unsafe extern "C" fn module_init(api: *const KernelApi) -> i32
unsafe extern "C" fn module_exit()   // optional: run by rmmod
unsafe extern "C" fn module_rescan() // optional: probe for new devices after a /proc/pci rescan
```

`KernelApi` (`modules/abi`) is a `#[repr(C)]` table, ABI v30 (append-only; v30 added a filesystem's `file_id` and `set_times_ino` hooks (the `*_ino` hooks are how an open file is reached) and `fd_lockctl` (file locks for the Linux layer); v29 added a filesystem's `set_size` and `set_size_ino` hooks (`ftruncate`); v28 added the hooks that keep a file unlinked while it is held (`unlink_keep`, `read_ino`, `write_ino`, `stat_ino`, `forget_ino`); v27 added `thread_place`; v26 added a filesystem's `unmount` hook (`umount`); v25 added file times, a filesystem's `set_times` hook and `vfs_set_times`; v21 added `blk_unregister`, the service registry (`service_register` / `service_lookup`) modules reach each other through, module threads (`thread_spawn`), `wake` and the USB bus types, `docs/usb.md`; v19 took `ioctl` out, v20 added the `open` hook that makes a file exclusive, `/dev/console/kbd`). Kernel fills it and passes it to `module_init`. Drivers register what they provide: `blk_register` (block devices), `dev_register` (char devices: the directory `/dev/<name>/` with `data` and an optional text `ctl`, and a `poll` hook for `data`'s readiness), `fs_register` / `vfs_mount` (filesystems; a backend's optional `mmap` hook maps device memory, `docs/fb.md`, and its `poll` hook reports readiness for `poll`), `console_register` (screen + keyboard), `personality_register` (a foreign syscall ABI, see `docs/linux-compat.md`); `dt_mmio_find` gives a driver its memory-mapped devices from the device tree.

A module calls the table through its safe methods, one per entry with the
same name (`api.blk_read(dev, lba, &mut buf)` for
`(api.blk_read)(dev, lba, ptr, len)`): they take slices, `&str` and
references, and the kernel copies or checks what it is handed, so no
`unsafe` is needed. The few entries that cannot be safe (`dealloc`, the
saved registers of a syscall, the FP/SIMD save area) have no method. A
module keeps the table `module_init` received in a `static API: ApiCell`
(`API.get()` afterwards), and every module crate denies
`unsafe_op_in_unsafe_fn`.

### Adding a module
1. Copy `modules/hello` → `modules/foo` (keep panic=abort, opt-level=s, myos-abi, link flags)
2. Add it to the module list in `kernel/build.rs` (builds `target/foo-<triple>` for every arch) and to `BOOT_MODULES` in `src/limine_image.rs` at the position it must load (that also ships it in the initramfs and generates the `module_path` line); list the ELFs in `scripts/ci-build-kernels.sh` / `ci-pack-build-artifacts.sh`
3. Or skip the boot list and load it on demand: `insmod /lib/modules/foo`

---

## Userspace (Summary)

### Syscalls (append-only)
A syscall takes up to six arguments in registers (x86-64: rdi, rsi, rdx, r10, r8, r9; aarch64: x0-x5; riscv64: a0-a5). The numbers only grow; a call that went keeps its number unused.

- **Path calls** (`kernel/src/user/at.rs`): every one names its file by a directory fd and a path relative to it, `*at` style. `AT_FDCWD` (-100) is the cwd, an absolute path leaves the fd out, and `AT_EMPTY_PATH` with an empty path is the fd's own file. The calls are `openat`, `statat` (64-bit size, access and modification times, the caller's rights as permission bits, the label owner's uid; `AT_SYMLINK_NOFOLLOW` for a symlink itself), `mknodat` (a directory or a FIFO), `symlinkat`, `unlinkat` (`AT_REMOVEDIR`), `renameat`, `readlinkat`, `utimensat` (seconds; tmpfs and ext2 keep times, the other filesystems report 0 and refuse), `chdirat`, `listdirat` (the names, one per line) and `execat` (an ELF, or a `#!` script run through its interpreter). libc's `fstat`, `futimens`, `fchdir`, `fdopendir` and `fexecve` are these on the fd itself, and the plain calls (`open`, `stat`, `chdir`, ...) are these on `AT_FDCWD`. Each call holds the VFS tree lock from resolving its paths to acting on them, so a directory fd stands for its directory whatever is renamed meanwhile. The cwd is a directory node too: it follows a rename of its directory. A directory fd whose directory the caller's namespace cannot name is a capability: paths resolve beneath it only, with the rights it was opened with (`docs/security.md`). `openat` with `O_CREAT|O_EXCL` creates a new file or fails (`EEXIST`) in one step under the tree lock, so of several processes racing for a name exactly one gets it, and a symlink at the name counts as taken; `O_CLOEXEC` makes the fd close at exec; `O_NOFOLLOW` refuses a symlink as the last component (`ELOOP`) and `O_DIRECTORY` anything but a directory (`ENOTDIR`); a directory opened to write, create or truncate is `EISDIR`; so a walk of a tree with `openat` cannot be led out of it by a symlink swapped in.
- **File I/O**: `pread(fd, buf, len, offset, flags)` and `pwrite` are read and write too: at the file position, which advances, or with `FILE_AT` at `offset`, the position left as it is (`ESPIPE` on a pipe or terminal; a write past the end leaves zeros in the gap). `ftruncate(fd, size)` cuts a file open for writing or grows it with zeros (tmpfs; ext2 through the module's `set_size` hook, the blocks past the end given back). `fdflags(fd, op, flags)` gets or sets an fd's close-on-exec flag (`fcntl`'s `F_GETFD`/`F_SETFD`); `dupfd` and `pipe` take it as a flag too (`F_DUPFD_CLOEXEC`, `pipe2(O_CLOEXEC)`). Exec closes the fds that have it; fork keeps it and `dup2`'s copy does not get it.
- **File locks**: `flock(fd, op)` (`LOCK_SH`, `LOCK_EX`, `LOCK_UN`, `| LOCK_NB`) and `lockctl(fd, cmd, range)`, a record lock on a byte range (get the first conflicting one, set, or set waiting; with `LOCKCTL_OFD` the open file description's rather than the process's): libc's `flock`, `fcntl`'s `F_GETLK`/`F_SETLK`/`F_SETLKW` and `lockf`, Rust's `File::lock` and `try_lock`.
- **The rest**: `exit`, `close`, `fork`, `wait`, `brk`, `pipe`, `dup2`, `execname`, `dupfd`, `getcwd`, `mmap`, `munmap`, `mprotect`, `lseek`, `poll`, …, threads (`thread_spawn`, `thread_exit`, `wait_addr`, `wake_addr`, `gettid`, see `docs/threads.md`), `getppid`, `mount`, `umount`, `settimeofday` (sets the wall clock until the next boot; the RTC keeps its time), and the security calls (`docs/security.md`): `setuser` (run as a user, checked against the password), `ns` (narrow the caller's namespace; libc's `chroot` is a namespace of one binding) and `policy_load`.
- **Gone**: the path calls before `*at` (`open`, `exec`, `listdir`, `stat`, `stat2`, `stat3`, `chdir`, `mkdir`, `rmdir`, `unlink`, `rename`, `symlink`, `readlink`, `mkfifo`, `utimens`, `futimens`) and `chroot`; `read` and `write` (numbers 3 and 0): `pread` and `pwrite` without `FILE_AT`; `ioctl` (number 28): a device's state is its `ctl` file (`docs/tty.md`).

### Init & Shell
`user/init` = PID1: baked in, smoke-tests fork/`/ok`, forks `/netd`, forks `/u/getty` and `wait()`/respawns. Getty prompts `login: ` → execs `/u/login` → the kernel checks the password (`setuser`; `root` has none) and the process becomes that user, in its login domain (`root`: `admin`) → execs `/sh`. `/sh` = oksh 7.9 with PATH `/bin/sbase:/bin/coreutils:/bin/ubase:/bin/custom:/bin/tcc:/bin/std:/bin/etc`. Editor: `vim` → `/bin/custom/vim` (FEAT_TINY; see `packages/vim/README.md`) and VCS: `git` → `/bin/custom/git` (Phase-1 local porcelain; see `packages/git/README.md`) are packages, `get-myos vim git` installs them. Framebuffer CSI includes scroll regions; `TERMCAP=/lib/termcap` (`ports/termcap`) + termios raw mode for full-screen TUI. A terminal is a directory, `data` and `ctl` (its termios and window size as text), the console at `/dev/console/`, the ptys at `/dev/pts/N/` from `/dev/pts/clone`, with `/proc/self/fd/N` and `/proc/self/tty` naming them (`docs/tty.md`).

### Rust Userspace
Syscall 9 (`brk`) backs per-process heap. `user/lib` exposes `brk`, `heap_init`, bump `GlobalAlloc`. `user/ok` smoke-tests every boot. `user/heap` = CI-only heavy suite. `std` programs link prebuilt sysroot (`toolchain/std/build-sysroot.sh`). Its `std::fs` creates, writes (truncate, append, `create_new`), seeks and reads files, gives an open file's metadata, renames, removes, makes directories and symlinks; it has no hard links, permissions or `canonicalize`. The kernel has one failure value, so each call works out its `ErrorKind` (`NotFound`, `AlreadyExists`, `IsADirectory`, `ReadOnlyFilesystem`, ...) the way libgloss does; reads and writes tell EIO and EINTR apart (`/bin/std/fs` checks them).

### C Userspace (newlib + libgloss)
Links against newlib with myos libgloss (syscall adapters + ENOSYS stubs). `stat` fills `st_atime`/`st_mtime` (`st_ctime` = `st_mtime`), and `utimensat`, `futimens`, `utimes`, `futimes`, `lutimes` and `utime` set them. `settimeofday` and `clock_settime` (`CLOCK_REALTIME`) set the wall clock (sbase `date MMDDhhmm[[CC]YY]`); `gethostname`, `sethostname` and `uname`'s node name read and write `/proc/sys/kernel/hostname`; `syslog` has no daemon behind it: `openlog`/`syslog`/`vsyslog` write each message as one line `ident[pid]: message` to the console (`/dev/console/data`), and to stderr as well with `LOG_PERROR` (sbase `logger`). Without kernel syscalls of their own: `readv`/`writev` (`<sys/uio.h>`), `vfork`, `daemon` and `getrandom` (from `/dev/urandom`) are libc over the existing primitives, the netdb service lookups find nothing (no services database), and the pthread API is there for single-threaded programs (`pthread_create` fails, `docs/threads.md`; an empty `libpthread.a` keeps `-lpthread` linking). A port's `myos_compat.h` / `myos_stubs.c` holds only what newlib's headers lack or what has no honest implementation on myos (rlimits, `getrusage`, `alarm`: no timer signals); what libgloss provides is never redefined there.

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
