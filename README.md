# myos

**A small, readable operating system written in Rust.**

myos boots through [Limine](https://github.com/limine-bootloader/limine) on
**x86_64** (BIOS and UEFI), **AArch64** and **RISC-V** (riscv64). It runs
in QEMU, and its disk image can be written to a USB stick for a real
machine. It gives you a login, a shell, a
C and Rust userspace, networking, sound and a graphical desktop with a web
browser. The kernel is `#![no_std]` Rust; drivers and filesystems are
loadable modules.

It is meant to be read and grown: the code aims to stay plain, and the
pages under [`docs/`](docs/) explain each part.

## What it has

- **Three architectures** from one tree, booted by Limine (protocol
  revision 6), with SMP and preemptive scheduling
  ([scheduler](docs/scheduler.md), [threads](docs/threads.md)).
- **A shell**: getty, login (`root` with an empty password), then
  [oksh](https://github.com/ibara/oksh), with ten text screens on the
  framebuffer and a serial console ([ttys](docs/tty.md)).
- **Security without a superuser**: every process runs for a user in a
  domain, every file has a label, and `/etc/policy` says what each domain
  may do with each label. Per-process namespaces narrow what a program can
  see ([security](docs/security.md)).
- **Kernel modules** for every driver and filesystem, loaded at boot or
  with `insmod` ([modules](docs/modules.md)): virtio, NVMe, SD cards, USB
  (xHCI, EHCI, UHCI, OHCI and the Pi's DWC2, with hubs, sticks, mice,
  keyboards and speakers), FAT and ext2 ([filesystems](docs/fs.md),
  [USB](docs/usb.md)).
- **Networking**: virtio-net in the kernel, the TCP/IP stack (smoltcp) in
  a userspace daemon, IPv4 and IPv6, BSD sockets over Plan 9 style `/net`
  files, `AF_UNIX` sockets, curl with HTTPS, dropbear SSH
  ([networking](docs/net.md), [Unix sockets](docs/sockets-unix.md),
  [HTTPS](docs/https.md), [SSH](docs/ssh.md)). Wi-Fi is in progress
  ([Wi-Fi](docs/wifi.md)).
- **Hardware support** through PCI, ACPI and device trees, with MSI and
  an IOMMU confining userspace drivers ([PCI](docs/pci.md),
  [interrupts](docs/interrupts.md), [IOMMU](docs/iommu.md),
  [platform](docs/platform.md)).
- **Sound**: virtio-sound and USB speakers, mixed by a sound server
  ([sound](docs/sound.md)).
- **Graphics**: a framebuffer device, an X server (TinyX), the dwm window
  manager, the st terminal and the surf web browser on WebKitGTK, all
  installable packages ([framebuffer](docs/fb.md),
  [packages](docs/packages.md)).
- **Userspace**: Rust programs (with a `std` port), C with newlib or
  mlibc, C++, and ported programs such as sbase, ubase, ripgrep, TinyCC,
  vim, git and Python ([userspace](docs/userspace.md),
  [ports](docs/ports.md), [mlibc](docs/mlibc.md)).
- **Packages**: programs the image does not carry, installed as apps with
  `get-myos` and run with `run-myos` ([packages](docs/packages.md)).
- **Linux binaries**: an optional syscall layer runs musl programs and
  Alpine packages ([Linux layer](docs/linux-compat.md)).
- **Install and upgrade** on a disk with two boot slots
  ([install](docs/install.md)), and power off or reboot cleanly
  ([power](docs/power.md)).
- **Boot tests** in CI on every architecture and machine variant
  ([testing](docs/testing.md)).

## Prerequisites

- [rustup](https://rustup.rs/). `rust-toolchain.toml` pins
  **nightly-2026-07-26** and installs its components automatically.
- QEMU for the three architectures, with the AArch64 and RISC-V UEFI
  firmware. CI runs Ubuntu 26.04's QEMU 10.2. The riscv64 boot gets its
  IOMMU only with QEMU 9.2 or later (`-device riscv-iommu-pci`); with
  24.04's QEMU 8.2 it boots without one, and the launcher says so once.
- The tools the build and the boot tests use:

| Tool | Used for |
|------|----------|
| `clang`, `lld` | cross-compiling and linking the C userspace on every arch (`ld.lld`); clang 19 or later for mlibc (C++23) |
| `make` | building newlib |
| `meson`, `ninja-build` | building mlibc (`toolchain/mlibc`, [mlibc](docs/mlibc.md)) |
| `cmake` | building the C++ runtimes against mlibc: libc++, libc++abi, libunwind (`toolchain/libcxx`, [mlibc](docs/mlibc.md) "C++") |
| `git` | fetching the upstream sources of the ports |
| `gh` | the GitHub CLI, for pulling CI's cached ports |
| `libc6-dev` | the host's C runtime (`Scrt1.o`) for the Limine host tool |
| `rsync` | preparing the oksh and sbase build trees |
| `patch` | applying the `*.myos.patch` files |
| `curl` | fetching the pinned Limine binary on the first build |
| `xorriso` | the hybrid ISO (`cargo run -- iso` only) |
| `zstd` | compressing the boot archives (`src/initramfs.rs`) and unpacking CI's cached ports (`scripts/ci-registry.sh pull`) |
| `e2fsprogs` | the boot test checks the scratch disk's ext2 with `e2fsck` |
| `dosfstools`, `mtools` | `cargo test -p fatvol`, and the boot test's `fsck.fat` of the FAT test disk |
| `openssh-client`, `python3` | the boot test's host side (`host.sh`): SSH sessions into the guest's dropbear, and the TCP and QEMU-monitor peers |

On Ubuntu 26.04 (on 24.04, use `qemu-system-misc` instead of
`qemu-system-riscv`):

```sh
sudo apt install qemu-system-x86 qemu-system-arm qemu-system-riscv \
  qemu-efi-aarch64 qemu-efi-riscv64 clang lld make meson ninja-build git gh \
  libc6-dev rsync patch curl xorriso zstd e2fsprogs dosfstools mtools \
  openssh-client python3
```

Building every port from source takes hours the first time. Pulling the
ones CI already built takes about a minute: see "Fast local setup" in
[`AGENTS.md`](AGENTS.md), which also covers sandboxes behind a proxy. For
macOS, see [running](docs/running.md).

## Quick start

```sh
cargo run
```

This builds the x86_64 kernel and a boot disk (GPT with a FAT ESP, BIOS
and UEFI bootable), writes the test disk `target/fat.img` and starts QEMU.
You see `Hello from myos`, then a login prompt: log in as `root` with an
empty password. Close the window to stop.

The first build also cross-builds everything the image carries: every
port with a `port.env` under `ports/`, `user/` and `toolchain/`, newlib
included ([ports](docs/ports.md)). That takes a while. A missing piece is a
build error, never a silently smaller image.

```sh
cargo run -- uefi        # x86_64 UEFI
cargo run -- aarch64     # AArch64 (AAVMF firmware; serial only)
cargo run -- riscv64     # RISC-V (EDK2 firmware)
cargo run -- iso         # write target/myos-x86_64.iso
cargo run --release      # a release build
```

On x86 you can type in the QEMU window (PS/2 keyboard) or on the serial
console; the other architectures use the serial console.

The boot tests boot headless, log in and run the tests inside the guest
([testing](docs/testing.md)):

```sh
cargo run -- test-mini            # the quick tests (no network)
cargo run -- uefi test-mini
cargo run -- aarch64 test         # every test: packages, HTTPS, SSH, os-test (needs the network)
```

For a Raspberry Pi, an installed disk that keeps its data between boots,
and running on real hardware, see [running](docs/running.md) and
[Raspberry Pi](docs/raspberry-pi.md).

## How it fits together

```
Limine (BIOS or UEFI firmware)
  └─ the kernel (higher half, #![no_std]) and the initramfs
       ├─ memory: the frame allocator, the kernel heap (a quarter of RAM, 64 MiB to 1 GiB)
       ├─ the scheduler: kernel threads and user processes on every CPU
       ├─ the VFS: rootfs, tmpfs, devfs, procfs, and FAT, ext2 and /net from modules
       ├─ the modules from the initramfs, in order: console, PCI, ACPI, disks, sound,
       │    USB, network, filesystems (pcidev and pl061 load only with insmod)
       └─ userspace
            ├─ init: mounts the disks, starts netd (TCP/IP) and sndd (sound)
            ├─ /etc/rc in the background: /data/etc/rc.local (the Linux layer, dropbear on :2222)
            └─ a getty on the serial port and on each text screen → login → oksh
```

[layout](docs/layout.md) lists what is where in the source tree, and
[running](docs/running.md) describes the boot, the images the build
writes and the memory layout.

## Documentation

| Page | What it covers |
|------|----------------|
| [running](docs/running.md) | building, the images, running in QEMU, persistent disks, real hardware, the memory layout |
| [layout](docs/layout.md) | the source tree: what each directory and key file is |
| [testing](docs/testing.md) | the boot tests, their tags, writing a test, CI |
| [ports](docs/ports.md) | ported programs: the `port.env` descriptor, building, adding a port |
| [packages](docs/packages.md) | packages, `get-myos` and `run-myos`, the mirror |
| [install](docs/install.md) | the boot disk layout, installing, upgrading, the two slots |
| [modules](docs/modules.md) | kernel modules, the module ABI, `insmod` and `rmmod` |
| [fs](docs/fs.md) | the VFS, mounts, rootfs, tmpfs, devfs, disks, caches, FAT and ext2 |
| [proc](docs/proc.md) | the files under `/proc` |
| [scheduler](docs/scheduler.md) | scheduling, SMP, blocking waits and idle CPUs |
| [threads](docs/threads.md) | threads, process lifetime, kernel stacks |
| [signals](docs/signals.md) | signals |
| [events](docs/events.md) | eventfd, timerfd, epoll and signalfd |
| [security](docs/security.md) | users, domains, labels, the policy, namespaces |
| [tty](docs/tty.md) | the console, text screens, ttys and ptys, the keyboard and mouse |
| [keymap](docs/keymap.md) | keyboard maps |
| [fb](docs/fb.md) | the framebuffer device |
| [sound](docs/sound.md) | the sound core, the sound server, `play` and `rec` |
| [usb](docs/usb.md) | USB: the core, the host controllers, the class drivers, userspace drivers |
| [pci](docs/pci.md) | PCI: the module split, BARs, DMA, userspace PCI drivers |
| [interrupts](docs/interrupts.md) | interrupt controllers and domains, device interrupts, MSI |
| [iommu](docs/iommu.md) | the IOMMU on each architecture |
| [platform](docs/platform.md) | the board description from ACPI and device trees, AML |
| [power](docs/power.md) | power off, reboot, halt |
| [net](docs/net.md) | networking: netd, `/net`, TCP and UDP, sockets, curl |
| [sockets-unix](docs/sockets-unix.md) | `/net/unix` and `AF_UNIX` sockets |
| [https](docs/https.md) | TLS and the CA bundle |
| [ssh](docs/ssh.md) | dropbear |
| [wifi](docs/wifi.md) | Wi-Fi: the daemon, the drivers, the simulated radio |
| [linux-compat](docs/linux-compat.md) | the Linux syscall layer, Alpine packages |
| [mlibc](docs/mlibc.md) | mlibc, the second C library, and C++ |
| [raspberry-pi](docs/raspberry-pi.md) | the Raspberry Pi 3 and 4 |

## License

myos's own code is dual-licensed under [Apache-2.0](LICENSE-APACHE) or
[MIT](LICENSE-MIT), at your option.

A built image also contains third-party programs under their own licenses
(some copyleft: Git, Lynx, GNU Make, TinyCC). See
[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) for the full list, the
source offer and the license texts in [`licenses/`](licenses/); the same
files are copied into the root of the ISO built by `cargo run -- iso`.
