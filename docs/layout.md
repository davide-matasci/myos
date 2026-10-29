# The source tree

## What is where

| Path | Role |
|------|------|
| `src/main.rs` | Host launcher: QEMU (BIOS/UEFI/AArch64/RISC-V) + second virtio-blk disk |
| `src/limine_image.rs` | Limine fetch + `limine.conf` + the images; the boot disk in `src/limine_disk.rs` (GPT, FAT32 ESP through `fatvol`, ext2 through `ext2fs`) |
| `build.rs` | Fetch Limine; wrap x86_64 kernel in BIOS+UEFI images; write `fat.img` |
| `kernel/src/main.rs` | `#![no_std]` Limine entry: heap, IRQs, scheduler, rootfs, Limine modules, user init |
| `kernel/src/limine_boot.rs` | Limine requests (HHDM, memmap, DTB, FB, modules, executable addr) |
| `kernel/src/platform.rs` | The board description, filled once at boot from the ACPI static tables (`acpi.rs`: MADT, MCFG, SPCR, GTDT) and the device tree (`dt.rs`, `fdt` crate), shown by `/proc/platform` (`docs/pci-acpi-smp.md`) |
| `kernel/src/dt.rs` | Device tree (aarch64, riscv64): fills the platform description; PCI INTx `interrupt-map`, `virtio,mmio` nodes |
| `kernel/src/mm.rs` | Physical frame allocator (after 256 KiB heap; page tables, user pages, virtqueues) |
| `kernel/src/power.rs` | Power-off, reboot, halt: stop the processes, unmount the disks, then the modules' and the arch's methods in turn (`docs/power.md`) |
| `kernel/src/sec/` | The security policy (`docs/security.md`): parser, labels, domains, users, the checks the syscalls make, the passwords' SHA-256 |
| `etc/policy` | The default policy: packed as `/etc/policy`, and the kernel's fallback copy |
| `kernel/src/blk.rs` | Block-device registry filled by driver modules (`blk_register`); `/dev/<name>` + sector/byte I/O, through the block cache (`blk/cache.rs`) |
| `kernel/src/arch/` | All per-arch code: boot, UART, interrupts, PCI, user entry/paging (`user`, `upaging`), context switch, FPU, clock, SMP glue |
| `kernel/src/console.rs` | Serial console + the `console` module's screen/keyboard hooks (early-output replay) |
| `kernel/src/input.rs` | Stdin line discipline: module keyboard + serial → fd 0 |
| `kernel/src/heap.rs` | `linked_list_allocator` heap sized from memory, with a free list per size class for small blocks (also holds tmpfs data) |
| `kernel/src/task/` | Scheduler records (`Task`) + per-process blocks (`process.rs`): yield, preemption, fork/exec/wait |
| `kernel/src/fs/` | VFS + rootfs/tmpfs/devfs/procfs backends |
| `kernel/src/modules/` | ELF64 loader, KernelApi wrappers, loaded-module registry |
| `modules/abi` | Shared `#[repr(C)]` KernelApi (v14: PCI/DMA/`dev_register`/`blk_register`/`console_register`/`personality_register`/`dt_mmio_find`) |
| `modules/virtio` | What the virtio modules share: the `virtio-drivers` crate's `Hal` over the module ABI, PCI and virtio-mmio probing by device type, interrupt routing |
| `modules/virtq` | Split virtqueue helpers for the console's virtio-input keyboard and pointers |
| `modules/console` | Framebuffer text screen and `/dev/fb`, PS/2 + virtio-input keyboards and the `keyboard` service other modules' keyboards type through, the pointer (`/dev/console/mouse`) with the PS/2 and virtio-input mice, loadable keymap (`keymaps/`; scancode decoding in the host-testable `ps2-scancode` crate) |
| `modules/virtio_blk` | virtio-blk `/dev/vd*` on `virtio-drivers`: virtio-pci (modern) or virtio-mmio |
| `modules/virtio_scsi` | virtio-scsi: the host's disks as `/dev/sd*` (a VPS's disks), on `virtio-drivers`' virtqueue |
| `modules/nvme` | NVMe `/dev/nvmeXn1` (PCI class 01/08, polled queues) |
| `modules/rpi_firmware` | The Raspberry Pi's VideoCore firmware through its mailbox property interface, as a service: power domains, clocks (`docs/raspberry-pi.md`); `/proc/firmware` |
| `modules/sdmmc` | SD cards `/dev/mmcblkN`: a standard SDHCI controller (PCI class 08/05, the Pi 4's EMMC2) or the Raspberry Pi 3's sdhost, the card layer on the `sdio-host` crate; high speed (50 MHz) when the card takes it, the blocks by ADMA2 on SDHCI, waits sleeping on the controller's interrupt, the Pi's clocks from its firmware. SDIO cards (the Pi's Wi-Fi chip, powered through the firmware) as `/dev/sdio/N` for a driver that is a program, the card layer the host-tested `sdio` crate (`docs/raspberry-pi.md`) |
| `modules/usb` | The USB core: the devices, their enumeration and hot-plug on its own kernel thread, the class drivers' bus service (`UsbHostOps`), `/proc/usb`; the host controller drivers register their controllers with it (`UsbHcdOps`, `docs/usb.md`) |
| `modules/xhci` | xHCI USB host controller driver: rings, slots, contexts and the transfers of its controllers, for the core |
| `modules/ehci` | EHCI (USB 2.0) host controller driver: its schedules and the transfers of its high-speed devices (isochronous OUT too), the full- and low-speed ones handed to its companions, for the core (`docs/usb.md`) |
| `modules/uhci` | UHCI (USB 1.1) host controller driver, an EHCI's companion: its frame list, the transfers, isochronous OUT, for the core (`docs/usb.md`) |
| `modules/ohci` | OHCI (USB 1.1) host controller driver, the companion of non-Intel EHCIs: its ED lists, the transfers, isochronous OUT, for the core (`docs/usb.md`) |
| `modules/dwc2` | DWC2 USB host controller driver, the Raspberry Pi 3's: host channels, split transactions, for the core (`docs/raspberry-pi.md`) |
| `modules/usb_hub` | USB hub class driver: ports, resets, the devices behind a hub |
| `modules/usb_storage` | USB mass storage (bulk-only, SCSI): `/dev/sdX/`, gone with the stick |
| `modules/usb_hid` | USB HID mice and tablets: the console's pointer, `/dev/console/mouse` (`docs/tty.md`) |
| `modules/usb_audio` | USB Audio Class 1 and 2 playback: a playback stream of the sound core over an isochronous OUT endpoint (`docs/usb.md`, `docs/sound.md`) |
| `modules/sound` | The sound core: the cards' PCM streams as `/dev/sound/N/{ctl,data}`, their buffers, the `sound` service the card drivers register with (`docs/sound.md`) |
| `modules/virtio_sound` | virtio-sound cards (PCI or MMIO): their playback and capture streams, for the sound core |
| `modules/usbdev` | Userspace USB drivers: `/dev/usb`, the interfaces no kernel driver takes, their control, bulk and interrupt transfers as files (`docs/usb.md`) |
| `modules/pcidev` | Userspace PCI drivers: `/dev/pci`, a function's registers, DMA memory and interrupt, confined by the IOMMU where the board has one (`docs/pci-acpi-smp.md`); shipped for `insmod /lib/modules/pcidev`, not loaded at boot |
| `modules/pl061` | The ARM PL061 GPIO controller's lines as an interrupt domain (`docs/pci-acpi-smp.md`, "Interrupt domains"); shipped for `insmod /lib/modules/pl061`, not loaded at boot |
| `modules/usb_hid` | USB HID mice and tablets, the console's pointer (`/dev/console/mouse`), and boot keyboards, the console's keyboard (`docs/tty.md`, `docs/usb.md`) |
| `modules/tap` | Virtual NICs: `/dev/tap/N`, a NIC whose wire is a program (the Wi-Fi daemon's link, `docs/wifi.md`) |
| `modules/hello` | Sample module (`[ OK ] hello`) |
| `modules/fat` | Writable FAT16/FAT32: `ModuleVfsOps` over the `fatvol` crate (`modules/fat/fatvol`: fstool's FAT driver, with growing by zeros, rename and file times of its own), host-tested against dosfstools and mtools |
| `modules/ext2` | Writable ext2: `ModuleVfsOps` over the `ext2fs` crate (`modules/ext2/ext2fs`, also `mkfs.ext2`'s), host-tested against e2fsprogs |
| `modules/virtio_net` | virtio-net on `virtio-drivers` (virtio-pci or virtio-mmio): `/dev/net0/` (`data` Ethernet frames, `ctl` the MAC and interrupt), RX interrupt wakes `poll` |
| `modules/netfs` | Plan 9 `/net` + `/dev/netd/data` channel to userspace netd; `/net/unix` local connections |
| `modules/linux` | Linux syscall compatibility layer: a syscall *personality* (`personality_register`) for musl binaries |
| `user/init` | PID1: smoke fork/`/ok`, `mount -a`, fork `/netd`, the sound server `sndd` and the boot script `/etc/rc` (`etc/rc`, in the background), a getty on `ttyS0` and each tty `/etc/ttys` names, respawned |
| `user/sh` | Legacy tiny shell (not `/sh`; kept in-tree) |
| `user/ok` | Slim always-on boot smoke (alloc/user/fat/proc) |
| `user/heap` | CI-only heavy smoke (std/C/sbase/ripgrep/tcc/bigalloc; uutils and git in the full mode), in the tests module (`docs/testing.md`) |
| `user/sndd` | The sound server: mixes what the programs play (the `sndd` local socket) into each playback stream, converts the capture streams for whoever records (`docs/sound.md`) |
| `user/netd` | Userspace smoltcp over `/dev/net0/data` and the virtual NICs of `/dev/tap` |
| `user/insmod` | `insmod /lib/modules/<name>`: load a kernel module at runtime (`SYS_INSMOD`) |
| `user/rmmod` | `rmmod <name>`: unload a kernel module that provides nothing any more (`SYS_RMMOD`) |
| `user/lib` | Shared `myos_user` syscall wrappers, argv parser, `Heap` allocator |
| `user/c` | Native C programs (newlib): `hello` and the boot-CI smokes as `/bin/etc/*`, in the tests module (`docs/testing.md`); `sec`, which `/etc/rc` runs, and `play`/`rec` (`docs/sound.md`) in the image |
| `user/c-mlibc` | The same smokes built against mlibc, `/bin/etc/mlibc/*` in the tests module, and their tests (`docs/mlibc.md`) |
| `user/echo/cat/ls` | Bootfs demos (`/myos_echo`, `/myos_cat`, `/myos_ls`) |
| `user/std` | The Rust `std` demo programs (`/bin/std/{hello,cat,echo,bigalloc}`) |
| `user/get-myos` | `get-myos`: installs packages (ports the image does not carry) from a mirror as apps, each version in a directory of its own, and `run-myos` runs one in a namespace with its files at their image paths; `-d` removes one, `--sync` makes the apps what `ROOT/apps.list` says, every change is a generation that `--rollback` returns to, and `--global` binds an app over the system for every process (its programs on `PATH`), `docs/packages.md`; `--upgrade` and `--install` write the boot disk's slots and Limine, `docs/install.md` |
| `user/mount` | `mount` prints `/proc/mounts` or issues `SYS_MOUNT` (`mount SRC TARGET FSTYPE`, `bind` for a bind mount); says why a mount failed |
| `user/umount` | `umount DIR` detaches the disk mounted there (`SYS_UMOUNT`) |
| `user/power` | `poweroff`, `reboot`, `halt` (one program, the action its name says; `SYS_POWER`, `docs/power.md`) |
| `ports/` | Userspace ports in the image: source fetched at build (sbase, ubase, oksh, ripgrep, curl, dropbear, limine, ...), one `port.env` descriptor each (`docs/ports.md`) |
| `packages/` | Ports CI builds and publishes but the image does not carry (vim, git, lynx, lua, make, os-test, x11-libs, tinyx, x11-xft, x11-fonts, dwm, st, dmenu, x11-apps, bottom, clear, get-alpine, nginx, python, rust; `get-myos NAME` installs them, `docs/packages.md`); moving a directory here (or back to `ports/`) is the whole change |
| `toolchain/newlib/` | newlib 4.4.0 + libgloss/myos syscall adapters |
| `toolchain/libmyos/` | The C both libcs share: the kernel's interface as libc functions |
| `toolchain/mlibc/` | mlibc 7.0.0, the second C library (opt-in per port, `docs/mlibc.md`) |
| `toolchain/std/` | Rust `std` PAL skeleton, sysroot build scripts (the `sysroot` port) |
| `targets/` | Custom Rust target specs (`x86_64-unknown-myos`, `aarch64-unknown-myos`, `riscv64imac-unknown-myos`) |
| `scripts/` | Thin wrappers for port builds; `ports.sh` (the descriptors); CI registry (`myos-c-userspace-lib.sh`) |
| `linux-compat/` | Userspace of the Linux layer: the `linux` launcher (every image), musl tests (feature `linux_compat`); `get-alpine` is a package (`packages/get-alpine`) |
