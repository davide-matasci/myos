# myos

**A minimal, readable operating system kernel written in Rust.**

myos boots in QEMU on **x86_64**, **AArch64**, and **RISC-V** through [Limine](https://github.com/limine-bootloader/limine), and reaches a fully interactive shell with login, userspace programs, and a modular VFS — in under 33k lines of Rust and C.

This is a starting point to grow into a real OS, not a feature dump.

---

## Features

- **Multi-arch boot** — x86_64 (BIOS + UEFI), AArch64, RISC-V via Limine protocol revision 6
- **Interactive shell** — getty → login (`root`, empty password) → [oksh](https://github.com/ibara/oksh) 7.9
- **Rust kernel** — `#![no_std]`, higher-half link, HHDM memory, preemptive round-robin scheduler
- **Kernel modules** — one ELF loader; FAT16, ext2, virtio-net, netfs are loadable
- **VFS with multiple backends** — bootfs, tmpfs, devfs, procfs, FAT16, ext2
- **Userspace ELFs** — Rust `#![no_std]` programs + Rust `std` smoke + full newlib/libgloss C toolchain
- **Ported userspace** — sbase, ubase, uutils coreutils, ripgrep, TinyCC (all fetched at build)
- **Networking** — virtio-net kernel module + smoltcp in userspace; `/ping` works on all arches
- **CI** — GitHub Actions with rust-cache; userspace port outputs are OCI artifacts on GHCR

---

## Prerequisites

- [rustup](https://rustup.rs/) — `rust-toolchain.toml` pins **nightly-2026-07-26** and installs components automatically
- QEMU (`qemu-system-x86`, `qemu-system-arm`, `qemu-efi-aarch64`, `qemu-efi-riscv64`)
- A C compiler (`cc`/`clang`) for the Limine host tool (`bios-install`)
- `curl` to fetch Limine binary on first build
- `xorriso` for hybrid ISO output

On Ubuntu: `sudo apt install qemu-system-x86 qemu-system-arm qemu-efi-aarch64 gcc xorriso`  
On macOS: `brew install qemu`

---

## Quick Start

```sh
cargo run
```

Builds x86_64 kernel, wraps in Limine GPT+FAT ESP (BIOS + UEFI), writes `target/fat.img`, starts QEMU. You'll see `Hello from myos`; close window to exit.

```sh
cargo run -- uefi        # x86_64 UEFI
cargo run -- aarch64     # AArch64 (AAVMF)
cargo run -- riscv64     # RISC-V (Edk2)
cargo run -- iso         # write target/myos-x86_64.iso
cargo run --release      # release build
```

**Headless / CI mode** — add `-- --ci` to kill QEMU after all boot needles:
```sh
cargo run -- --ci
cargo run -- uefi --ci
cargo run -- aarch64 --ci
```

Expected headless output (x86_64 / AArch64):
```
Hello from myos
heap ok
int ok
task a
task b
sched ok
mod ok
limine mod ok
sh ok
user ok
fat ok
```

**Interactive use** — type at `$` prompt (PS/2 keyboard in QEMU window on x86; serial on all arches):
```sh
cargo run          # x86 BIOS — keyboard or serial
cargo run -- uefi  # x86 UEFI
cargo run -- aarch64   # serial only for now
```
CI types `root` at `login: `, then commands at `$ ` (password is empty).

---

## Architecture Overview

```
Boot (Limine)
  └─ Limine HHDM + memmap + framebuffer + modules
       └─ Kernel (higher-half, #![no_std])
            ├─ Heap (256 KiB linked-list allocator)
            ├─ Scheduler (round-robin kernel threads + user tasks)
            ├─ VFS (mount table → bootfs / tmpfs / devfs / procfs / ext2 / netfs)
            ├─ virtio-blk (in-kernel, x86 PCI legacy / AArch64 MMIO)
            ├─ Modules: FAT16, ext2, virtio-net, netfs
            └─ Userspace (ELF processes)
                 ├─ /ok smoke (always-on alloc/user/fat/disk/proc markers)
                 ├─ /netd (smoltcp over /dev/net0; only opener of net0)
                 ├─ getty → login → /sh (oksh 7.9 via newlib/libgloss)
                 └─ CI /heap: std / C / sbase / uutils / ripgrep / tcc
```

### Boot
Limine protocol base revision 6 (`limine` crate 0.6.5). Host tool fetches pinned Limine `v12.6.1`, writes GPT+FAT ESP, `limine.conf`, kernel ELF and module ELFs. On x86, `limine bios-install` makes the image BIOS+UEFI bootable. No `bootloader` crate, no QEMU `-kernel`, no Multiboot.

### Memory
Kernel linked in higher half (`0xffffffff80000000` on x86_64). Limine provides HHDM; usable memory = `phys + HHDM`. Page tables allocated from bump allocator after heap. AArch64 device block (UART, GIC, virtio-mmio) identity-mapped via `TTBR0`.

### Scheduling
Round-robin kernel threads + user tasks. `task::yield_now()` cooperative; timer IRQ calls `task::schedule()` after EOI → preemptive even in user mode. x86_64: xAPIC timer. AArch64: GICv2 physical timer (PPI 30). RISC-V: ACLINT.

### Console & Input
Dual console: serial + Limine framebuffer (mirrored). Stdin (fd 0) merges PS/2 keyboard (x86, 8042 probe) and serial simultaneously.

---

## Repository Layout

| Path | Role |
|------|------|
| `src/main.rs` | Host launcher: QEMU (BIOS/UEFI/AArch64/RISC-V) + second virtio-blk disk |
| `src/limine_image.rs` | GPT+FAT ESP writer + Limine fetch + `limine.conf` + `fat.img` |
| `build.rs` | Fetch Limine; wrap x86_64 kernel in BIOS+UEFI images; write `fat.img` |
| `kernel/src/main.rs` | `#![no_std]` Limine entry: heap, IRQs, scheduler, bootfs, modules, virtio-blk, fat, user init |
| `kernel/src/limine_boot.rs` | Limine requests (HHDM, memmap, DTB, FB, modules, executable addr) |
| `kernel/src/mm.rs` | Physical frame allocator (after 256 KiB heap; page tables, user pages, virtqueues) |
| `kernel/src/blk.rs` | In-kernel virtio-blk: legacy I/O-BAR (x86) and virtio-mmio (AArch64) |
| `kernel/src/arch/` | x86_64, AArch64, RISC-V arch code (UART, GIC/PCI, PSCI, etc.) |
| `kernel/src/console.rs` | Dual console: serial + framebuffer mirror |
| `kernel/src/input.rs` | Stdin ring buffer: PS/2 keyboard + serial → fd 0 |
| `kernel/src/heap.rs` | 256 KiB `linked_list_allocator` heap |
| `kernel/src/task/` | Round-robin threads + user tasks: yield, preemption, fork/exec/wait |
| `kernel/src/fs/` | VFS + bootfs/tmpfs/devfs/procfs backends |
| `kernel/src/modules/` | ELF64 loader, KernelApi wrappers, loaded-module registry |
| `modules/abi` | Shared `#[repr(C)]` KernelApi (v7: PCI/DMA/`dev_register`) |
| `modules/hello` | Sample module: embedded + ESP via Limine |
| `modules/stubfs` | Sample prefixed mount via `vfs_mount` at `/disk` |
| `modules/fat` | FAT16 kernel module: `blk_read` + `vfs_register("msg")` |
| `modules/ext2` | Writable ext2 (rev1, 1 KiB blocks): `ModuleVfsOps` |
| `modules/virtio_net` | Modern virtio-pci net: poll RX/TX, `/dev/net0` Ethernet frames |
| `modules/netfs` | Plan 9 `/net` + `/dev/netd` channel to userspace netd |
| `user/init` | PID1: smoke fork/`/ok`, fork `/netd`, exec `/sh` (baked in) |
| `user/sh` | Legacy tiny shell (not `/sh`; kept in-tree) |
| `user/ok` | Slim always-on boot smoke (alloc/user/fat/disk/proc) |
| `user/heap` | CI-only heavy smoke (std/C/sbase/uutils/ripgrep/tcc/bigalloc) |
| `user/netd` | Userspace smoltcp over `/dev/net0` |
| `user/lib` | Shared `myos_user` syscall wrappers, argv parser, `Heap` allocator |
| `user/echo/cat/ls` | Bootfs demos (`/myos_echo`, `/myos_cat`, `/myos_ls`) |
| `user/mount` | `mount` prints `/proc/mounts` or issues `SYS_MOUNT` |
| `ports/` | Userspace ports: source fetched at build (sbase, ubase, oksh, ripgrep, coreutils, tcc) |
| `toolchain/newlib/` | newlib 4.4.0 + libgloss/myos syscall adapters |
| `toolchain/std/` | Rust `std` PAL skeleton, sysroot build scripts |
| `targets/` | Custom Rust target specs (`x86_64-unknown-myos`, `aarch64-unknown-myos`, `riscv64imac-unknown-myos`) |
| `scripts/` | Thin wrappers for port builds; CI registry (`myos-c-userspace-lib.sh`) |

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
Attached as second virtio-blk in all QEMU runs. Holds `/msg` for `fat ok` marker. Bare metal without it still boots to shell.

---

## Real Hardware

Write the Limine disk image to USB/internal drive (`target/bios.img` for BIOS, `target/uefi.img` for UEFI). Framebuffer mirrors serial — boot progress scrolls on screen.

**stdin** merges PS/2 keyboard and serial. If 8042 probe succeeds, `kbd ok` prints and keyboard works. Serial always available.

| Arch | Serial port | Baud rate |
|------|-------------|-----------|
| x86_64 | COM1 (I/O `0x3F8`) | 38400 8N1 |
| AArch64 | PL011 UART | 115200 8N1 |
| RISC-V | UART | 115200 8N1 |

---

## VFS & Filesystems (Summary)

- **VFS** — mount table with longest-prefix routing; `vfs::mounts_text()` exports `/proc/mounts`
- **bootfs** — read-only embedded namespace at `/`; Limine ESP modules override; demos use `myos_` prefix
- **procfs** — `/proc/mounts` (generated, not stored bytes)
- **tmpfs/devfs** — writable mount for `O_CREAT`; device nodes
- **virtio-blk** — in-kernel (chicken/egg for FAT); x86 PCI legacy, AArch64 virtio-mmio
- **FAT16 module** — parses BPB, walks cluster chain, registers `/msg` from root `MSG`
- **ext2 module** — writable, rev1, 1 KiB blocks, bound via `mount(2)` fstype `ext2`
- **virtio-net / netfs / netd** — kernel virtio-net → `/dev/net0` Ethernet; netfs mounts Plan 9 `/net`; netd runs smoltcp in userspace over `/dev/netd`; `/ping <ipv4>` uses `/net/icmp`

---

## Kernel Modules

Kernel modules are ELFs the kernel already has in RAM. One loader copies `PT_LOAD`, applies relocs, calls `module_init`.

Two ways to get bytes into RAM:

| | Embedded | Limine |
|---|---|---|
| Bytes live in | kernel binary (`include_bytes!`) | file on ESP |
| Rebuild trigger | kernel build | disk image rebuild |

Module exports:
```rust
unsafe extern "C" fn module_init(api: *const KernelApi) -> i32
unsafe extern "C" fn module_exit() // optional
```

`KernelApi` (`modules/abi`) is `#[repr(C)]` table, ABI v7 (append-only). Kernel fills it and passes to `module_init`.

### Adding a module
1. Copy `modules/hello` → `modules/foo` (keep panic=abort, opt-level=s, myos-abi, link flags)
2. **Embedded:** add nested `cargo build` in `kernel/build.rs`, `include_bytes!`, call `modules::load("foo", IMAGE)` after heap/IRQs
3. **Limine:** build ELF, copy to `target/`, add `module_path: boot():/boot/foo` to `LIMINE_CONF` in `src/limine_image.rs`, rebuild disk image

---

## Userspace (Summary)

### Syscalls (append-only)
`write`, `exit`, `open`, `read` (fd 0 = keyboard+serial), `close`, `exec`, `fork`, `wait`, `listdir`, `brk`, `pipe`, `dup2`, `stat`, `execname`, `dupfd`, `chdir`, `getcwd`, `mkdir`, `rmdir`, `unlink`, `rename`, `symlink`, `readlink`, `mmap`, `munmap`, `mprotect`, `lseek`.

### Init & Shell
`user/init` = PID1: baked in, smoke-tests fork/`/ok`, forks `/netd`, forks `/u/getty` and `wait()`/respawns. Getty prompts `login: ` → execs `/u/login` → accepts `root`/empty → execs `/sh`. `/sh` = oksh 7.9 with PATH `/s:/c:/`.

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
./ports/tcc/build.sh                # TinyCC as /t/tcc (-run support)
./ports/ripgrep/build.sh            # ripgrep + PCRE2 as /c/rg
```

Implemented libgloss hooks call real syscalls where they exist; stubs return ENOSYS/EROFS for rest.

---

## CI

GitHub Actions caches Cargo with Swatinem/rust-cache (`prefix-key: limine-8.3-6`). Userspace port outputs are OCI artifacts on GHCR, one package per port, tagged with stamp hash from `scripts/myos-c-userspace-lib.sh`. First run after stamp change = miss + push; later runs with same hashes = hit. Packages should be **public** for fork PRs. Source checkouts (`*-src`) are never cached.

---

## Notes

- x86_64: CPU halts with `hlt` (QEMU stays open; `--ci` attaches `isa-debug-exit` for clean exit)
- AArch64: kernel issues PSCI `SYSTEM_OFF` (QEMU treats as shutdown)
- Kernel linked in higher half; Limine sets stack, enables MMU, provides HHDM
- AArch64 device MMIO identity-mapped on `TTBR0` (not in HHDM at Limine base rev 3+)
- Modules run from HHDM heap (rwx); loader flushes I-cache on AArch64 after copy
- Limine binaries downloaded from GitHub release `v12.6.1` (sha256-pinned) into `target/limine-v12.6.1`
- `user/ok` exits early if `/msg` absent (FAT disk not present); CI fails on `fat ok`