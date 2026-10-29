# The source tree

This page lists what is where in the myos tree: every top-level directory
and the key files inside it. Use it to find the code behind a feature
before you read the subsystem's own page in `docs/`. The tables follow the
tree by area: the host side, the kernel, the modules, the native
userspace, and the toolchains and ports. Build output and fetched sources
all land in `target/`, which is never committed.

## Host side

The launcher, the image assembly and the files packed into every image.

| Path | Role |
|------|------|
| `src/main.rs` | Host launcher: QEMU (BIOS/UEFI/AArch64/RISC-V) + second virtio-blk disk, the test modes, `MYOS_DISK` / `MYOS_BOOT_DISK` (`docs/running.md`) |
| `src/limine_image.rs` | Limine fetch + `limine.conf` + the images |
| `src/limine_disk.rs`, `src/limine_gpt.rs`, `src/limine_fat.rs`, `src/limine_dir.rs` | The boot disk: GPT, FAT32 ESP through `fatvol`, ext2 through `ext2fs` (`limine_disk.rs`); the GPT header (`limine_gpt.rs`); the FAT16 data disk's writer (`limine_fat.rs`) and its directory lookup (`limine_dir.rs`) |
| `src/initramfs.rs` | The initramfs packer: a newc (cpio) archive of every image port's `PORT_FILES` for one arch, one zstd frame (`docs/ports.md`) |
| `src/ports.rs` | The port descriptors (`port.env`), read by `build.rs`, the packer and the kernel's build script (`docs/ports.md`) |
| `src/packages.rs` | The packages: one gzip tar per port with the files it would have in the image, and the indexes (`docs/packages.md`) |
| `src/release.rs` | The release a build belongs to, for the packages and the image (`lib/myos-release`), which `get-myos` compares with a mirror |
| `src/boot_test.rs` | The boot test's host side: boot headless, log in, start the guest's test runner, watch the console (`docs/testing.md`) |
| `src/test_www.rs` | The test web server the guest's network tests fetch from (`docs/testing.md`) |
| `src/sound.rs` | Sound in the QEMU boots: the virtio-sound cards, a USB speaker, and the check of what the tests played (`docs/sound.md`) |
| `src/usb2.rs` | USB 2.0 in the QEMU boots: EHCI with UHCI companions, beside the xHCI (`docs/usb.md`) |
| `src/wifi_sim.rs`, `src/wifi_net.rs` | The simulated Wi-Fi network of the test boots (issue #495): access points on the host, and the network behind them (`docs/wifi.md`) |
| `build.rs` | Fetch Limine; wrap x86_64 kernel in BIOS+UEFI images; write `fat.img` |
| `etc/policy` | The default security policy: packed as `/etc/policy`, and the kernel's fallback copy (`docs/security.md`) |
| `etc/rc`, `etc/rc.local`, `etc/ttys`, `etc/hosts`, `etc/services` | The rest of the image's `/etc`: the boot script init runs (`rc`), the default of the administrator's boot script `/data/etc/rc.local` (`docs/install.md`), the ttys init starts a getty on, the host and service names |
| `boot/rpi3` | The Raspberry Pi 3's ARM stub (`armstub8.S`) and U-Boot build (`docs/raspberry-pi.md`) |
| `targets/` | Custom Rust target specs (`x86_64-unknown-myos`, `aarch64-unknown-myos`, `riscv64-unknown-myos`) |
| `scripts/` | Thin wrappers for port builds; `ports.sh` (the descriptors); CI registry (`ci-registry.sh`) and the ports' hash and freshness functions (`myos-c-userspace-lib.sh`) |

## Kernel

The kernel's code, under `kernel/src/`.

| Path | Role |
|------|------|
| `kernel/src/main.rs` | `#![no_std]` Limine entry: heap, IRQs, scheduler, rootfs, Limine modules, user init |
| `kernel/src/limine_boot.rs` | Limine requests (HHDM, memmap, DTB, FB, modules, executable addr) |
| `kernel/src/platform.rs` | The board description, filled once at boot from the ACPI static tables (`acpi.rs`: MADT, MCFG, SPCR, GTDT) and the device tree (`dt.rs`, `fdt` crate), shown by `/proc/platform` (`docs/platform.md`) |
| `kernel/src/acpi.rs` | The ACPI static tables, from the RSDP Limine hands over: the platform description on x86_64 and on aarch64 firmware that has one (EDK2) |
| `kernel/src/dt.rs` | Device tree (aarch64, riscv64): fills the platform description; PCI INTx `interrupt-map`, `virtio,mmio` nodes |
| `kernel/src/mm.rs` | Physical frame allocator (after the kernel heap; page tables, user pages, virtqueues) |
| `kernel/src/heap.rs` | `linked_list_allocator` heap sized from memory, with a free list per size class for small blocks (also holds tmpfs data) |
| `kernel/src/arch/` | All per-arch code: boot, UART, interrupts, PCI, user entry/paging (`user`, `upaging`), context switch, FPU, clock, SMP glue |
| `kernel/src/smp.rs` | Multi-core bring-up via Limine MP, cross-CPU scheduling hooks |
| `kernel/src/irq.rs`, `kernel/src/irqchip.rs`, `kernel/src/msi.rs` | Interrupts: the handler registry, the root interrupt controllers, message-signalled interrupts (`docs/interrupts.md`) |
| `kernel/src/pci.rs`, `kernel/src/dma.rs`, `kernel/src/iommu/` | PCI configuration access for the module ABI; DMA per device; the IOMMUs (VT-d, AMD-Vi, SMMUv3, RISC-V) (`docs/pci.md`, `docs/iommu.md`) |
| `kernel/src/power.rs` | Power-off, reboot, halt: stop the processes, unmount the disks, then the modules' and the arch's methods in turn (`docs/power.md`) |
| `kernel/src/sec/` | The security policy (`docs/security.md`): parser, labels, domains, users, the checks the syscalls make, the passwords' SHA-256 |
| `kernel/src/blk.rs`, `kernel/src/blk/` | Block-device registry filled by driver modules (`blk_register`); `/dev/<name>` + sector/byte I/O, through the block cache (`blk/cache.rs`); the GUID partition table (`blk/gpt.rs`) |
| `kernel/src/console.rs` | Serial console + the `console` module's screen/keyboard hooks (early-output replay) |
| `kernel/src/input.rs` | Stdin line discipline: module keyboard + serial → fd 0 |
| `kernel/src/tty.rs`, `kernel/src/pty.rs` | The shared tty core (termios, line discipline) and the pty pairs of `/dev/pts` (`docs/tty.md`) |
| `kernel/src/pipe.rs` | In-kernel pipe ring buffers for `pipe(2)` |
| `kernel/src/evfd.rs` | Event counters, timers, wait sets and signal fds: `/dev/event/N`, `/dev/timer/N`, `/dev/wait/N`, `/dev/sig/N` (`docs/events.md`) |
| `kernel/src/signal.rs` | Signals: numbers, default actions, delivery (`docs/signals.md`) |
| `kernel/src/rng.rs` | The kernel CSPRNG (ChaCha20): `/dev/urandom`, `/dev/random` |
| `kernel/src/time.rs` | Wall-clock time from the platform RTC |
| `kernel/src/exception.rs` | Logs CPU exceptions to serial before halting |
| `kernel/src/personality.rs` | Foreign syscall personalities a module registers (the Linux layer) |
| `kernel/src/task/` | Scheduler records (`Task`) + per-process blocks (`process.rs`): yield, preemption, fork/exec/wait; also threads, signals' bits, process groups (`jobs.rs`), namespaces (`ns.rs`), CPU-time accounting (`acct.rs`) |
| `kernel/src/user/` | User mode: per-process page tables and address spaces, copy-on-write, image loading, the syscalls (`syscall.rs`) |
| `kernel/src/fs/` | VFS + rootfs/tmpfs/devfs/procfs backends; also ptsfs, evfs, the page cache, file locks, and the initramfs's cpio and zstd readers (`docs/fs.md`) |
| `kernel/src/modules/` | ELF64 loader, KernelApi wrappers, loaded-module registry (`docs/modules.md`) |

## Modules

Loadable kernel modules, under `modules/` (`docs/modules.md`).

| Path | Role |
|------|------|
| `modules/abi` | Shared `#[repr(C)]` KernelApi (`ABI_VERSION` 60: PCI/DMA/`dev_register`/`blk_register`/`console_register`/`personality_register`/`dt_mmio_find`, ...) |
| `modules/acpi` | Minimal ACPI: the tables' summary, the AML interpreter on x86_64 (the PCI interrupt routes, `_S5`), the ACPI power-off and reset methods (`docs/platform.md`, "AML") |
| `modules/pci_enum` | PCI enumeration → `/proc/pci` |
| `modules/virtio` | What the virtio modules share: the `virtio-drivers` crate's `Hal` over the module ABI, PCI and virtio-mmio probing by device type, interrupt routing |
| `modules/virtq` | Split virtqueue helpers for the console's virtio-input keyboard and pointers |
| `modules/console` | Framebuffer text screen and `/dev/fb`, PS/2 + virtio-input keyboards and the `keyboard` service other modules' keyboards type through, the pointer (`/dev/console/mouse`) with the PS/2 and virtio-input mice, loadable keymap (`keymaps/`; scancode decoding in the host-testable `ps2-scancode` crate) |
| `modules/virtio_blk` | virtio-blk `/dev/vd*` on `virtio-drivers`: virtio-pci (modern) or virtio-mmio |
| `modules/virtio_scsi` | virtio-scsi: the host's disks as `/dev/sd*` (a VPS's disks), on `virtio-drivers`' virtqueue |
| `modules/nvme` | NVMe `/dev/nvmeXn1` (PCI class 01/08, polled queues) |
| `modules/rpi_firmware` | The Raspberry Pi's VideoCore firmware through its mailbox property interface, as a service: power domains, clocks (`docs/raspberry-pi.md`); `/proc/firmware` |
| `modules/sdmmc` | SD cards `/dev/mmcblkN` and SDIO cards (below) |
| `modules/usb` | The USB core: the devices, their enumeration and hot-plug on its own kernel thread, the class drivers' bus service (`UsbHostOps`), `/proc/usb`; the host controller drivers register their controllers with it (`UsbHcdOps`, `docs/usb.md`) |
| `modules/xhci` | xHCI USB host controller driver: rings, slots, contexts and the transfers of its controllers, for the core |
| `modules/ehci` | EHCI (USB 2.0) host controller driver: its schedules and the transfers of its high-speed devices (isochronous OUT too), the full- and low-speed ones handed to its companions, for the core (`docs/usb.md`) |
| `modules/uhci` | UHCI (USB 1.1) host controller driver, an EHCI's companion: its frame list, the transfers, isochronous OUT, for the core (`docs/usb.md`) |
| `modules/ohci` | OHCI (USB 1.1) host controller driver, the companion of non-Intel EHCIs: its ED lists, the transfers, isochronous OUT, for the core (`docs/usb.md`) |
| `modules/dwc2` | DWC2 USB host controller driver, the Raspberry Pi 3's: host channels, split transactions, for the core (`docs/raspberry-pi.md`) |
| `modules/usb_hub` | USB hub class driver: ports, resets, the devices behind a hub |
| `modules/usb_storage` | USB mass storage (bulk-only, SCSI): `/dev/sdX/`, gone with the stick |
| `modules/usb_hid` | USB HID mice and tablets, the console's pointer (`/dev/console/mouse`), and boot keyboards, the console's keyboard (`docs/tty.md`, `docs/usb.md`) |
| `modules/usb_audio` | USB Audio Class 1 and 2 playback: a playback stream of the sound core over an isochronous OUT endpoint (`docs/usb.md`, `docs/sound.md`) |
| `modules/usbdev` | Userspace USB drivers: `/dev/usb`, the interfaces no kernel driver takes, their control, bulk and interrupt transfers as files (`docs/usb.md`) |
| `modules/sound` | The sound core: the cards' PCM streams as `/dev/sound/N/{ctl,data}`, their buffers, the `sound` service the card drivers register with (`docs/sound.md`) |
| `modules/virtio_sound` | virtio-sound cards (PCI or MMIO): their playback and capture streams, for the sound core |
| `modules/pcidev` | Userspace PCI drivers: `/dev/pci`, a function's registers, DMA memory and interrupt, confined by the IOMMU where the board has one (`docs/pci.md`); shipped for `insmod /lib/modules/pcidev`, not loaded at boot |
| `modules/pl061` | The ARM PL061 GPIO controller's lines as an interrupt domain (`docs/interrupts.md`, "Interrupt domains"); shipped for `insmod /lib/modules/pl061`, not loaded at boot |
| `modules/fat` | Writable FAT16/FAT32: `ModuleVfsOps` over the `fatvol` crate (`modules/fat/fatvol`: fstool's FAT driver, with growing by zeros, rename and file times of its own), host-tested against dosfstools and mtools |
| `modules/ext2` | Writable ext2: `ModuleVfsOps` over the `ext2fs` crate (`modules/ext2/ext2fs`, also `mkfs.ext2`'s), host-tested against e2fsprogs |
| `modules/virtio_net` | virtio-net on `virtio-drivers` (virtio-pci or virtio-mmio): `/dev/net0/` (`data` Ethernet frames, `ctl` the MAC and interrupt), RX interrupt wakes `poll` |
| `modules/tap` | Virtual NICs: `/dev/tap/N`, a NIC whose wire is a program (the Wi-Fi daemon's link, `docs/wifi.md`) |
| `modules/netfs` | Plan 9 `/net` + `/dev/netd/data` channel to userspace netd; `/net/unix` local connections |
| `modules/linux` | Linux syscall compatibility layer: a syscall *personality* (`personality_register`) for musl binaries |
| `modules/hello` | Sample module (`[ OK ] hello`) |

`modules/sdmmc` in detail (`docs/raspberry-pi.md`):

- The controllers: a standard SDHCI controller (PCI class 08/05, the Pi
  4's EMMC2) or the Raspberry Pi 3's sdhost.
- The card layer is on the `sdio-host` crate. It runs at high speed
  (50 MHz) when the card takes it, moves the blocks by ADMA2 on SDHCI,
  waits sleeping on the controller's interrupt, and gets the Pi's clocks
  from its firmware.
- SDIO cards (the Pi's Wi-Fi chip, powered through the firmware) appear
  as `/dev/sdio/N`, for a driver that is a program. Their card layer is
  the host-tested `sdio` crate.

## Native userspace

The programs under `user/`. Each has a `port.env` that names what it puts
in the image (`docs/ports.md`); most land in `/bin/custom`.

| Path | Role |
|------|------|
| `user/init` | PID1: smoke fork/`/bin/custom/ok`, `mount -a`, fork `/bin/custom/netd`, the sound server `sndd` and the boot script `/etc/rc` (`etc/rc`, in the background), a getty on `ttyS0` and each tty `/etc/ttys` names, respawned |
| `user/ok` | Slim always-on boot smoke (alloc/user/fat/proc) |
| `user/heap` | CI-only heavy smoke (std/C/sbase/ripgrep/tcc/bigalloc; uutils and git in the full mode), in the tests module (`docs/testing.md`) |
| `user/tests` | The boot tests' guest side: the runner (`run.sh`), the kernel's tests (`kernel.sh`), their host peer (`host.sh`) and the test CA (`docs/testing.md`) |
| `user/lib` | Shared `myos_user` syscall wrappers, argv parser, `Heap` allocator |
| `user/netd` | Userspace smoltcp over `/dev/net0/data` and the virtual NICs of `/dev/tap` |
| `user/net` | The smoltcp PHY library netd builds on, and `smoltcp/`: smoltcp's pin, myos's patches and `prepare.sh`, which lays the patched crate out in `target/` |
| `user/tls` | TLS client library: LibreSSL's libtls over a `/net` TCP data fd |
| `user/dns`, `user/http`, `user/ping` | `dns` (resolution over `/net/udp`), `http` (HTTP/HTTPS GET over `/net/tcp` and `user/tls`), `ping` (ICMP echo over `/net/icmp`) |
| `user/wifi` | The Wi-Fi daemon and its command (`docs/wifi.md`), with the host-tested radio and WPA crates (`bwfm`, `iwx`, `mtw`, `wpa`) and `wifi-firmware`, which fetches the radios' firmware |
| `user/sndd` | The sound server: mixes what the programs play (the `sndd` local socket) into each playback stream, converts the capture streams for whoever records (`docs/sound.md`) |
| `user/insmod` | `insmod /lib/modules/<name>`: load a kernel module at runtime (`SYS_INSMOD`) |
| `user/rmmod` | `rmmod <name>`: unload a kernel module that provides nothing any more (`SYS_RMMOD`) |
| `user/mount` | `mount` prints `/proc/mounts` or issues `SYS_MOUNT` (`mount SRC TARGET FSTYPE`, `bind` for a bind mount); says why a mount failed |
| `user/umount` | `umount DIR` detaches the disk mounted there (`SYS_UMOUNT`) |
| `user/mkfs.ext2`, `user/mkfs.fat` | Format a whole block device as ext2 (the `ext2fs` crate) or FAT32 (the `fatvol` crate) |
| `user/power` | `poweroff`, `reboot`, `halt` (one program, the action its name says; `SYS_POWER`, `docs/power.md`) |
| `user/ps` | `ps`: the processes in `/proc`, a shell script (`docs/proc.md`) |
| `user/get-myos` | `get-myos` and `run-myos`, the package manager (details below) |
| `user/echo`, `user/cat`, `user/ls` | Bootfs demos (`/bin/custom/echo`, `/bin/custom/cat`, `/bin/custom/ls`) |
| `user/sh` | Legacy tiny shell (not `/sh`; kept in-tree) |
| `user/std` | The Rust `std` demo programs (`/bin/std/{hello,cat,echo,bigalloc,sleep,fs,thread,alloc}`) |
| `user/c` | Native C programs (newlib): `hello` and the boot-CI smokes as `/bin/etc/*`, in the tests module (`docs/testing.md`); `sec`, which `/etc/rc` runs, and `play`/`rec` (`docs/sound.md`) in the image |
| `user/c-mlibc` | The same smokes built against mlibc, `/bin/etc/mlibc/*` in the tests module, and their tests (`docs/mlibc.md`) |

`user/get-myos` in detail:

- `get-myos` installs packages (ports the image does not carry) from a
  mirror as apps, each version in a directory of its own
  (`docs/packages.md`).
- `run-myos` runs one in a namespace, with its files at their image paths.
- `-d` removes one. `--sync` makes the apps what `ROOT/apps.list` says.
- Every change is a generation that `--rollback` returns to.
- `--global` binds an app over the system for every process, with its
  programs on `PATH`.
- `--upgrade` and `--install` write the boot disk's slots and Limine
  (`docs/install.md`).

## Toolchains, ports and packages

| Path | Role |
|------|------|
| `toolchain/newlib/` | newlib 4.4.0 + libgloss/myos syscall adapters |
| `toolchain/libmyos/` | The C both libcs share: the kernel's interface as libc functions |
| `toolchain/mlibc/` | mlibc 7.0.0, the second C library (opt-in per port, `docs/mlibc.md`) |
| `toolchain/libcxx/` | LLVM's C++ runtimes against mlibc: libc++, libc++abi, libunwind (`docs/mlibc.md`, "C++") |
| `toolchain/std/` | Rust `std` PAL skeleton, sysroot build scripts (the `sysroot` port) |
| `ports/` | Userspace ports in the image: source fetched at build (sbase, ubase, oksh, ripgrep, curl, dropbear, limine, ...), one `port.env` descriptor each (`docs/ports.md`) |
| `packages/` | Ports CI builds and publishes but the image does not carry (see below); `get-myos NAME` installs them (`docs/packages.md`). Moving a directory here (or back to `ports/`) is the whole change |
| `linux-compat/` | Userspace of the Linux layer: the `linux` launcher (every image), musl tests (feature `linux_compat`); `get-alpine` is a package (`packages/get-alpine`) |

The packages: coreutils, vim, git, lynx, lua, make, os-test, x11-libs,
tinyx, x11-xft, x11-fonts, dwm, st, dmenu, x11-apps, bottom, clear,
get-alpine, nginx, openssl, acme, python, rust, tcc, ncurses, pcre2,
sqlite, libedit, glib, image-libs, cairo-pango, gtk3, libsoup, webkitgtk,
surf.
