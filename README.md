# myos

**A minimal, readable operating system kernel written in Rust.**

myos boots in QEMU on **x86_64**, **AArch64**, and **RISC-V** through [Limine](https://github.com/limine-bootloader/limine), and reaches a fully interactive shell with a login prompt, userspace programs, and a modular VFS — in under 33 000 lines of Rust and C.

This is a starting point to grow into a real OS, not a feature dump. Everything is kept readable and self-contained.

---

## Features

- **Multi-arch boot** — x86_64 (BIOS + UEFI), AArch64, and RISC-V all boot through Limine protocol revision 6
- **Interactive shell** — getty → login (`root`, empty password) → [oksh](https://github.com/ibara/oksh) 7.9 (portable OpenBSD ksh)
- **Rust kernel** — `#![no_std]`, higher-half link, HHDM memory, preemptive round-robin scheduler
- **Kernel modules** — one ELF loader for all modules; FAT16, ext2, virtio-net, and netfs are all loadable
- **VFS with multiple backends** — bootfs (embedded ELFs), tmpfs, devfs, procfs, FAT16, ext2
- **Userspace ELFs** — Rust `#![no_std]` programs baked in or loaded from ESP; also Rust `std` smoke and full newlib/libgloss C toolchain
- **Ported userspace** — sbase (~91 utilities), ubase (getty/login), uutils coreutils, ripgrep, TinyCC, all fetched and built at compile time
- **Networking** — virtio-net kernel module + smoltcp in userspace via `/dev/net0`; `/ping` works on all arches
- **CI** — GitHub Actions with rust-cache; userspace port outputs are OCI artifacts on GHCR

---

## Prerequisites

- [rustup](https://rustup.rs/) — `rust-toolchain.toml` pins **nightly-2026-07-26** and installs the needed components automatically
- QEMU (`qemu-system-x86`, `qemu-system-arm`, `qemu-efi-aarch64`, `qemu-efi-riscv64`)
- A C compiler (`cc`) for the Limine host tool (`bios-install`)
- `curl` to fetch the Limine binary tarball on first build
- `xorriso` for hybrid ISO output (`cargo run -- iso`)

Nightly components (installed automatically from `rust-toolchain.toml`):

```
llvm-tools-preview
rust-src
x86_64-unknown-none
aarch64-unknown-none-softfloat
```

On Ubuntu:

```sh
sudo apt install qemu-system-x86 qemu-system-arm qemu-efi-aarch64 gcc xorriso
```

On macOS:

```sh
brew install qemu
```

---

## Quick Start

```sh
cargo run
```

This builds the x86_64 kernel, wraps it in a Limine GPT+FAT ESP (BIOS + UEFI), writes `target/fat.img` as a second virtio-blk disk, and starts QEMU. You should see green `Hello from myos` on a black screen; close the window to exit.

```sh
cargo run -- uefi        # same image over UEFI (OVMF fetched on first run)
cargo run -- aarch64     # AArch64 on qemu-system-aarch64 + AAVMF
cargo run -- riscv64     # RISC-V on qemu-system-riscv64 + RISC-V UEFI firmware
cargo run -- iso         # write target/myos-x86_64.iso (needs xorriso)
cargo run --release      # release builds (smaller, faster)
```

**Headless / CI mode** — the same commands with `-- --ci` at the end kill QEMU once all boot needles are seen (BIOS/UEFI/AArch64 all work headless):

```sh
cargo run -- --ci
cargo run -- uefi --ci
cargo run -- aarch64 --ci
```

Expected headless output on x86_64 and AArch64:
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

On RISC-V the full needle set is still being brought up; the boot kernel smoke (`sh ok`, `user ok`) is required.

**Interactive use** — type at the `$` prompt (PS/2 keyboard in the QEMU window on x86; serial on all arches):

```sh
# x86 BIOS — keyboard or serial at $
cargo run

# x86 UEFI
cargo run -- uefi

# AArch64 — serial only for now
cargo run -- aarch64
```

CI types `root` at `login: `, then commands at `$ ` (password is empty).

---

## Architecture Overview

```
Boot (Limine)
  └─ Limine HHDM + memory map + framebuffer + modules
       └─ Kernel (higher-half, #![no_std])
            ├─ Heap (256 KiB linked-list allocator)
            ├─ Scheduler (round-robin kernel threads + user tasks)
            ├─ VFS (mount table → bootfs / tmpfs / devfs / procfs / ext2 / netfs)
            ├─ virtio-blk (in-kernel, x86 PCI legacy / AArch64 MMIO)
            ├─ Modules: FAT16, ext2, virtio-net, netfs
            └─ Userspace (ELF processes)
                 ├─ /ok smoke (always-on, alloc / user / fat / disk / proc markers)
                 ├─ /netd (smoltcp over /dev/net0; only opener of net0)
                 ├─ getty → login → /sh (oksh 7.9 via newlib/libgloss)
                 └─ CI /heap: std / C / sbase / uutils / ripgrep / tcc
```

### Boot

Limine protocol base revision 6 (`limine` crate 0.6.5). The host tool fetches pinned Limine binary `v12.6.1`, writes a GPT+FAT ESP (`BOOTX64.EFI` / `BOOTAA64.EFI`, `BOOTRISCV64.EFI`), writes `limine.conf`, and copies the kernel ELF and any module ELFs onto the ESP. On x86, `limine bios-install` is run so the image boots natively on both BIOS and UEFI machines.

No `bootloader` crate. No QEMU `-kernel`. No Multiboot.

### Memory

Kernel is linked in the higher half (`0xffffffff80000000` on x86_64). Limine provides an HHDM offset; all usable memory is accessed as `phys + HHDM`. Page tables are allocated from the bump allocator after the heap.

On AArch64 the 1 GiB device block (UART at `0x09000000`, GICv2 at `0x08000000`, virtio-mmio at `0x0a000000`) is identity-mapped via `TTBR0` since it is not included in the HHDM at Limine base revision 3+.

### Scheduling

Round-robin kernel threads plus user tasks. Tasks call `task::yield_now()` cooperatively; the timer IRQ also calls `task::schedule()` after EOI, making the switch preemptive even in user mode.

x86_64: local xAPIC timer. AArch64: GICv2 generic physical timer (PPI 30). RISC-V: ACLINT.

### Interrupt handling

Each process has its own CR3/TTBR0 page table. The kernel/HHDM is mapped into every address space. On x86_64 the user page table has no kernel mappings (separate kernel CR3); on AArch64 TTBR0_EL1 points to the kernel page table and TTBR0_EL2 maps the device block.

### Console and input

Dual console: serial (COM1 / PL011) and Limine framebuffer (mirrored, so real hardware shows green text on screen). Stdin (fd 0) merges PS/2 keyboard (x86, 8042 probe) and serial simultaneously.

---

## Repository Layout

| Path | Role |
|------|------|
| `src/main.rs` | Host launcher: starts QEMU (BIOS/UEFI/AArch64/RISC-V) + second virtio-blk disk |
| `src/limine_image.rs` | GPT+FAT ESP writer + Limine binary fetch + `limine.conf` + `fat.img` |
| `build.rs` | Fetch Limine; wrap x86_64 kernel in BIOS+UEFI images; write `fat.img` |
| `kernel/src/main.rs` | `#![no_std]` Limine entry: heap, IRQs, scheduler, bootfs, modules, virtio-blk, fat, user init, halt |
| `kernel/src/limine_boot.rs` | Limine requests (HHDM, memmap, DTB, framebuffer, modules, executable addr) |
| `kernel/src/mm.rs` | Physical frame allocator (after the 256 KiB heap; page tables, user pages, virtqueues) |
| `kernel/src/blk.rs` | In-kernel virtio-blk: legacy I/O-BAR (x86) and virtio-mmio (AArch64) |
| `kernel/src/arch/x86/` | COM1, GDT/TSS/IDT/xAPIC, PCI (0xCF8/0xCFC), PS/2 keyboard |
| `kernel/src/arch/aarch64/` | PL011, GICv2, lower-EL SVC, virtio-mmio blk, PSCI off |
| `kernel/src/arch/riscv/` | CLINT, PLIC, UART16550 for RISC-V |
| `kernel/src/console.rs` | Dual console: serial + framebuffer mirror |
| `kernel/src/input.rs` | Stdin ring buffer: PS/2 keyboard + serial merged into fd 0 |
| `kernel/src/framebuffer.rs` | Pixel writer for a Limine framebuffer |
| `kernel/src/font.rs` | Tiny 8×8 bitmap font |
| `kernel/src/heap.rs` | 256 KiB `linked_list_allocator` heap from Limine usable + HHDM memory |
| `kernel/src/task/` | Round-robin kernel threads + user tasks: `yield_now`, timer preemption, fork/exec/wait syscalls |
| `kernel/src/fs/` | VFS (`vfs.rs`) + bootfs / tmpfs / devfs / procfs backends |
| `kernel/src/modules/` | ELF64 loader, `KernelApi` wrappers, loaded-module registry |
| `modules/abi` | Shared `#[repr(C)]` KernelApi (v7: PCI/DMA/`dev_register` after v6 VFS) |
| `modules/hello` | Sample module: built both embedded (`include_bytes!`) and on ESP via Limine |
| `modules/stubfs` | Sample prefixed mount via `vfs_mount` at `/disk` |
| `modules/fat` | FAT16 kernel module: `blk_read` + `vfs_register("msg")` from root `MSG` |
| `modules/ext2` | Writable ext2 (rev1, 1 KiB blocks): `ModuleVfsOps`, bound at `mount(2)` |
| `modules/virtio_net` | Modern virtio-pci net: poll-mode RX/TX, `/dev/net0` Ethernet frames |
| `modules/netfs` | Plan 9 `/net` + `/dev/netd` channel to userspace netd |
| `user/init` | PID1: smoke fork/`/ok`, fork `/netd`, exec `/sh`; `include_bytes!` baked in |
| `user/sh` | Legacy tiny shell (not `/sh`; kept in-tree for reference) |
| `user/ok` | Slim always-on boot smoke (alloc / user / fat / disk / proc); loaded from ESP |
| `user/heap` | CI-only heavy smoke (std/C/sbase/uutils/ripgrep/tcc/bigalloc); typed as `heap` at `$` |
| `user/netd` | Userspace smoltcp over `/dev/net0`; only process that opens net0 |
| `user/lib` | Shared `myos_user` syscall wrappers, argv parser, `Heap` allocator |
| `user/echo` | Print argv; installed as `/myos_echo` on bootfs |
| `user/cat` | Read a bootfs file; installed as `/myos_cat` |
| `user/ls` | List bootfs entries; installed as `/myos_ls` |
| `user/mount` | `mount` with no args prints `/proc/mounts`; `mount src tgt fstype` issues `SYS_MOUNT` |
| `ports/` | Userspace ports: source fetched at build time, not vendored |
| `ports/sbase/` | ~91 suckless sbase utilities; `.myos.patch` files, `bins.txt`, compat headers |
| `ports/ubase/` | getty + login; `.myos.patch` files |
| `ports/oksh/` | oksh 7.9; `pconfig.h` (no curses, no jobs, `--enable-small`), `.myos.patch` files |
| `ports/tcc/` | TinyCC; `tccrun` mmap/`-run` glue |
| `ports/ripgrep/` | ripgrep; PCRE2 support via `/c/rg` |
| `ports/coreutils/` | uutils coreutils multicall binaries under `/c/` |
| `toolchain/newlib/` | newlib 4.4.0 + libgloss/myos syscall adapters; fetch/build scripts |
| `toolchain/std/` | Rust `std` PAL skeleton, sysroot build scripts, upstream OSDev notes |
| `targets/` | Custom Rust target specs (`x86_64-unknown-myos`, `aarch64-unknown-myos`, `riscv64imac-unknown-myos`) |
| `scripts/` | Thin wrappers for all port builds; CI registry (`myos-c-userspace-lib.sh`) |

---

## Build & Run in Detail

### Build only

```sh
cargo build
```

Produces:
- `target/bios.img` — BIOS+UEFI hybrid disk
- `target/uefi.img` — UEFI-only ESP
- `target/fat.img` — 20 MiB FAT16 data disk (second virtio-blk device)
- `target/aarch64.img` — AArch64 ESP
- `target/riscv64.img` — RISC-V ESP

### x86_64 BIOS boot

```sh
qemu-system-x86_64 -m 256 \
  -drive format=raw,file=target/bios.img \
  -drive if=none,id=vd0,format=raw,file=target/fat.img \
  -device virtio-blk-pci,drive=vd0,disable-modern=on \
  -serial stdio
```

### x86_64 UEFI boot

```sh
qemu-system-x86_64 -m 256 \
  -drive format=raw,file=target/uefi.img \
  -drive if=none,id=vd0,format=raw,file=target/fat.img \
  -device virtio-blk-pci,drive=vd0,disable-modern=on \
  -serial stdio
```

### AArch64

```sh
qemu-system-aarch64 -m 256 \
  -cpu cortex-a72 \
  -machine virt,gic-version=2 \
  -drive format=raw,file=target/aarch64.img \
  -drive if=none,id=vd0,format=raw,file=target/fat.img \
  -device virtio-blk-device,drive=vd0 \
  -bios /usr/share/AAVMF/AAVMF_CODE.fd \
  -serial stdio
```

The launcher fetches OVFIM firmware (`ovmf-prebuilt`) on first run; distro AAVMF paths are also tried.

### RISC-V

```sh
qemu-system-riscv64 -m 256 \
  -machine virt \
  -drive format=raw,file=target/riscv64.img \
  -bios /usr/share/qemu/ovmf-bin/Edk2-riscv64.fd \
  -serial stdio
```

### ISO (hybrid BIOS+UEFI)

```sh
cargo run -- iso          # writes target/myos-x86_64.iso (no QEMU)
qemu-system-x86_64 -m 256 -cdrom target/myos-x86_64.iso -serial stdio
```

### Second disk (`target/fat.img`)

All QEMU invocations attach `target/fat.img` as a second virtio-blk disk. It is not the boot drive — it holds `/msg` so `user/ok` can print `fat ok`. Bare metal without this disk still boots to the shell.

---

## Real Hardware

Write the Limine disk image to a USB stick or internal drive as you would for QEMU (`target/bios.img` for BIOS, `target/uefi.img` for UEFI). On real hardware the kernel mirrors serial output to the framebuffer, so boot progress scrolls on screen as well as serial.

**stdin** merges PS/2 keyboard and serial. If the 8042 probe succeeds, `kbd ok` is printed and you can type at the shell prompt on a directly attached keyboard. Serial is always available.

| Arch | Serial port | Baud rate |
|------|-------------|-----------|
| x86_64 | COM1 (I/O `0x3F8`) | 38400 8N1 |
| AArch64 | PL011 UART (board-specific; `0x09000000` on QEMU virt) | 115200 8N1 |
| RISC-V | UART (board-specific; `0x10000000` on QEMU virt) | 115200 8N1 |

On many desktops COM1 is a motherboard header (9-pin) or rear DB9. Use a USB‑serial adapter, connect **GND/RX/TX**, and open the port in `picocom`, `minicom`, or PuTTY:

```sh
picocom -b 38400 /dev/ttyUSB0
```

If the monitor stops at `Hello from myos`, the OS was still running on serial only — connect serial or update to a build with framebuffer mirroring.

---

## VFS and Filesystems

### VFS layer

`kernel/src/fs/vfs.rs` holds a mount table. Each mount has a name (fstype), an optional path prefix, a source, and a set of operations (`lookup`, `stat`, `listdir`, `register`). The kernel routes all file syscalls through VFS, which picks the longest matching prefix. `vfs::mounts_text()` snapshots the table as Linux-shaped `source target fstype opts 0 0` lines.

### Bootfs

Read-only embedded namespace at `/`. User ELFs are registered at boot. Limine ESP modules add files by basename. Loadable modules register files via `KernelApi::vfs_register`. This is the first mount (prefix `""`).

Layout convention:
- Handwritten Rust demos: `myos_` prefix (`/myos_echo`, `/myos_cat`, `/myos_ls`)
- sbase: `/s/` (sbase `PREFIX /s`; bare `ls`/`cat` resolve here)
- uutils coreutils: `/c/` (multicall binaries)
- oksh: `/sh` (the shell)
- getty/login: `/u/getty`, `/u/login` (not on PATH)

### procfs

Read-only generated mount at `/proc`. The only node today is `/proc/mounts`. `cat /proc/mounts` or the `mount` command with no arguments prints the current mount table.

### tmpfs and devfs

Both are in-kernel backends. tmpfs is the default writable mount for `open` with `O_CREAT` (no storage backing). devfs is reserved for device nodes.

### virtio-blk

**In-kernel**, not loadable (chicken-and-egg: the FAT parser needs block I/O to load the FAT module). x86 uses PCI config space (`0xCF8`/`0xCFC`) to find VirtIO vendor/device and talks legacy I/O-BAR virtio-blk. AArch64 probes virtio-mmio v2 at `0x0a000000` (stride `0x200`, device id 2). Guest DMA uses HHDM VAs minus `hhdm_offset()` — frame physical addresses from `mm::alloc_frame`, not `kernel_virt_to_phys`.

### FAT16 module (`modules/fat`)

A kernel module using only `KernelApi` (`blk_read` / `vfs_register`): parses the BPB, scans the root directory for `MSG`, walks the FAT16 cluster chain, and registers `/msg`. Root-only, no subdirectories, no FAT32. The image at `target/fat.img` is FAT16 so the existing writer stays in the cluster range.

### ext2 module (`modules/ext2`)

A kernel module like FAT16, but writable. Uses `ModuleVfsOps` with byte-granular `blk_read_at`/`blk_write_at`. Bound at `mount(2)` with `fstype ext2`.

### virtio-net, netfs, and netd

**virtio-net** (`modules/virtio_net`) is a modern virtio-pci network module: poll-mode RX/TX, registers `/dev/net0` as raw Ethernet frames. No IP stack in the kernel.

**netfs** (`modules/netfs`) mounts Plan 9 `/net` (`tcp`/`udp`/`icmp` clone + `conv` of `ctl`/`data`/`status`) and registers `/dev/netd`.

**netd** (`user/netd`) is the only process that opens `/dev/net0`. It runs smoltcp in userspace (DHCP + ICMP + UDP + TCP) and talks to the kernel over `/dev/netd` using poll-style reads (no `socket()` syscall, no pipe IPC).

**`/ping <ipv4>`** uses `/net/icmp` only. CI pings `10.0.2.2` (the QEMU SLIRP gateway). Interactive use can ping `1.1.1.1`.

QEMU adds `-netdev user,id=net0 -device virtio-net-pci,netdev=net0` on every arch (replacing the default NIC with `-nic none`).

---

## Kernel Modules

Kernel modules are ELFs the kernel already has in RAM. One loader (`kernel/src/modules`) copies `PT_LOAD`, applies relocations, and calls `module_init`. They are not loaded through `open`/`exec` — the kernel already holds their bytes.

Two ways to get bytes into RAM:

| | Embedded | Limine |
|---|---|---|
| Bytes live in | the kernel binary (`include_bytes!`) | a file on the ESP |
| Who maps them | the compiler | Limine, from `module_path` in `limine.conf` |
| Kernel hook | `modules::load("name", IMAGE)` | `modules::load_limine_modules()` (auto-walks every Limine module) |
| Rebuild trigger | kernel build | disk image rebuild |
| `/msg` at boot | yes | no (limine copy would print `mod ok` twice) |

A module exports:

```rust
unsafe extern "C" fn module_init(api: *const KernelApi) -> i32
unsafe extern "C" fn module_exit() // optional
```

`KernelApi` (`modules/abi`) is a `#[repr(C)]` table. ABI version is **7**. Pointers are appended, never reordered. The kernel fills it and passes `&KernelApi` to `module_init`.

Modules must not call kernel internals. There is no dynamic linker.

### Adding a new module

Copy `modules/hello` to `modules/foo`. Keep `panic = "abort"`, `opt-level = "s"`, `myos-abi = { path = "../abi" }`, and the link flags in `build.rs` (`-z max-page-size=4096`, `-u module_init`, `-pie -nostdlib` on x86 only).

**Embedded:** add a nested `cargo build` in `kernel/build.rs`, then `const FOO_IMAGE: &[u8] = include_bytes!(env!("FOO_MODULE_PATH"))` in `kernel/src/modules/mod.rs`, and call `modules::load("foo", FOO_IMAGE)` after heap and IRQs are up.

**Limine:** build the ELF, copy it to `target/` with the expected name, add `module_path: boot():/boot/foo` to `LIMINE_CONF` in `src/limine_image.rs`, then rebuild the disk image. No kernel rebuild needed.

---

## Userspace

### Syscall table

Syscall numbers are append-only. Errors return `usize::MAX`.

| nr | name | Description |
|----|------|-------------|
| 0 | write | fd, ptr, len |
| 1 | exit | exit code |
| 2 | open | path, flags → fd (cwd-aware) |
| 3 | read | fd, buf, len (fd 0 = keyboard + serial) |
| 4 | close | fd |
| 5 | exec | path, argv, envp (0 or `[argc, (ptr,len)...][envc, ...]`) |
| 6 | fork | → child pid (parent), 0 (child) |
| 7 | wait | status_ptr → reaped pid; stores exit byte if ptr set |
| 8 | listdir | path, buf → byte count (cwd-aware; newline-separated) |
| 9 | brk | addr → program break (0 = query) |
| 10 | pipe | fds_ptr → 0 or error |
| 11 | dup2 | oldfd, newfd |
| 12 | stat | path, out_ptr → 0 or error |
| 13 | execname | buf, len → basename length |
| 14 | dupfd | oldfd, minfd |
| 15 | chdir | path → 0 or error (per-task cwd) |
| 16 | getcwd | buf, len → pathname length (NUL written) |
| 17 | mkdir | path → 0 or error (writable mount) |
| 18 | rmdir | path → 0 or error |
| 19 | unlink | path → 0 or error |
| 20 | rename | old, new → 0 or error |
| 21 | symlink | target, link → 0 or error |
| 22 | readlink | path, buf → byte count |
| 23 | mmap | addr, len, prot, flags, fd, offset → va or error |
| 24 | munmap | addr, len → 0 or error |
| 25 | mprotect | addr, len, prot → 0 or error |
| 26 | lseek | fd, offset, whence → position |

x86_64: `syscall`, `rax`=nr, `rdi/rsi/rdx`=a0/a1/a2. AArch64: `svc`, `x8`=nr, `x0/x1/x2`=a0/a1/a2. At `_start`, argc is in `x0` and argv in `x1` (AArch64) or on the user stack (x86_64, System V).

### Init and the shell

`user/init` is PID1: baked in with `include_bytes!`, spawned at boot. It smoke-tests fork and slim `/ok`, forks `/netd`, then forks `/u/getty` and `wait()`s/respawns it. Getty prompts `login: ` and execs `/u/login`; login accepts `root` with an empty password and execs `/sh`. `/sh` is oksh 7.9 with PATH `/s:/c:/`.

The tiny `user/sh` crate stays in-tree but is not `/sh`.

### Rust userspace heap and `std`

Syscall 9 (`brk`) backs a per-process heap. `user/lib` exposes `brk`, `heap_init`, and a bump `GlobalAlloc`. `user/ok` smoke-tests it every boot (`alloc ok`). `user/heap` runs the heavy CI suite.

To build `std` programs, prebuild the sysroot:

```sh
./toolchain/std/build-sysroot.sh    # builds sysroot for x86_64 and AArch64
```

Then link against it (`toolchain/std/toolchain/config.toml.example` shows the `.cargo/config.toml` needed). More syscalls are still needed for real `std` programs.

### C userspace (newlib + libgloss)

C programs link against newlib with a myos libgloss port (syscall adapters + ENOSYS stubs). No new kernel syscalls are needed.

```sh
./toolchain/newlib/build.sh    # fetch newlib 4.4.0, build libc + libgloss/myos
./scripts/build-c-hello.sh     # minimal write() smoke test
./ports/sbase/build.sh          # ~91 sbase utilities under /s/
./ports/ubase/build.sh          # getty + login under /u/
./ports/oksh/build.sh           # oksh 7.9 as /sh
./ports/tcc/build.sh            # TinyCC as /t/tcc (-run support)
./ports/ripgrep/build.sh        # ripgrep + PCRE2 as /c/rg
```

**Implemented libgloss hooks** call real syscalls where they exist: `write`, `read`, `open` (writable on tmpfs/devfs), `close`, `brk`, `fork`, `wait`/`waitpid`, `pipe`, `dup2`, `execve`, `stat`, `chdir`/`getcwd`, `mkdir`/`rmdir`/`unlink`/`rename`/`symlink`/`readlink` (on writable mounts), `mmap`/`munmap`/`mprotect`, `lseek`. Stubs return ENOSYS/EROFS for unimplemented calls.

CI on all arches checks `sbase ok` from `/s/echo` and `sls ok` from `/s/ls` via `/heap`.

---

## CI

GitHub Actions caches Cargo's registry and `target` with Swatinem/rust-cache (`prefix-key: limine-8.3-6`). Userspace port **outputs** (newlib prefixes, sbase/oksh/ubase/ripgrep/tcc/uutils ELFs, stamps) are OCI artifacts on GHCR, one package per port, tagged with the stamp hash from `scripts/myos-c-userspace-lib.sh`. The first run after a stamp change is a cache miss; later runs with the same hashes hit. Packages should be **public** so fork PRs can pull.

Source checkouts (`*-src`) are never cached.

---

## Notes

- On x86_64 the CPU halts with `hlt` after boot (QEMU stays open; `--ci` attaches `isa-debug-exit` so QEMU exits cleanly).
- On AArch64 the kernel issues PSCI `SYSTEM_OFF` after boot (QEMU treats this as a shutdown).
- The kernel is linked in the higher half. Limine sets the stack, enables the MMU, and provides the HHDM.
- AArch64 device MMIO is identity-mapped on `TTBR0` since it is not in the HHDM at Limine base revision 3+.
- Modules run from the HHDM heap (rwx). The loader flushes the I-cache on AArch64 after copying.
- Limine binaries are downloaded from GitHub release `v12.6.1` (sha256-pinned) into `target/limine-v12.6.1`. Not a git submodule.
- `user/ok` exits early if `/msg` is absent (the FAT disk is not present); CI then fails on `fat ok`.
