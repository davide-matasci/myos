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
- **Kernel modules** — one ELF loader; every driver and filesystem is a module (console, virtio-blk, virtio-scsi, NVMe, xHCI, EHCI, UHCI and OHCI USB with hubs, sticks, mice, keyboards and speakers, virtio-sound, virtio-net, netfs, FAT16/FAT32, ext2, …), and so is the Linux syscall layer; listed in `limine.conf` and loadable at runtime with `insmod`
- **VFS with multiple backends** — rootfs, tmpfs, devfs, procfs, FAT16/FAT32, ext2
- **Framebuffer** — `/dev/fb/ctl` (geometry, taking the screen from the console) and `/dev/fb/data` (pixels, `mmap(MAP_SHARED)`), served by the console module (`docs/fb.md`); the keyboard's presses and releases at `/dev/console/kbd` (`docs/tty.md`); an X server on both, TinyX's `Xfbdev` with MIT-SHM (`get-myos tinyx`, `packages/tinyx/README.md`), with antialiased TrueType text through Xft (`packages/x11-xft/README.md`) the dwm window manager, the st terminal and the dmenu menu dwm starts (`get-myos tinyx dwm` brings st, dmenu, the Xft stack and the fonts with it, then `run-myos tinyx,dwm:startx`; `packages/dwm/README.md`, `packages/st/README.md`, `packages/dmenu/README.md`); `xev` prints the events a window gets and `xprop` its properties (`get-myos x11-apps`, then `run-myos x11-apps`, `packages/x11-apps/README.md`); the surf web browser, on WebKitGTK (`get-myos surf`, then `run-myos surf URL` in dwm's terminal, `packages/surf/README.md`)
- **Userspace ELFs** — Rust `#![no_std]` programs + Rust `std` smoke + full newlib/libgloss C toolchain
- **Ported userspace** — sbase (one multicall ELF), ubase, ripgrep, TinyCC (all fetched at build); uutils coreutils is a package (`get-myos coreutils`, then `run-myos coreutils:ls`)
- **Networking** — virtio-net kernel module (RX interrupts: MSI-X on every arch, through the GICv3 ITS or a GICv2m frame on aarch64 and the AIA's IMSIC on riscv64; INTx on a board without one) + smoltcp in userspace, IPv4 (DHCP) and IPv6 (SLAAC; `AF_INET6` sockets, dual-stack as on Linux; `docs/sockets-curl.md`); `/ping` works on all arches; Wi-Fi under way (`wifi daemon`, `wifi scan`, `wifi connect` to an open or WPA2-PSK network carrying netd's traffic, over the test boots' simulated radio, Intel's AX201 or AX200 through `/dev/pci` (a port of OpenBSD's iwx), a MediaTek MT7601U or MT7612U USB adapter through `/dev/usb` (ports of OpenBSD's mtw and Linux's mt76x2u) or the Raspberry Pi's Broadcom CYW43455 or CYW43438 through `/dev/sdio` (a port of OpenBSD's bwfm, the firmware doing the 802.11 work), untried on the hardware, `docs/wifi.md`, issues #495 and #547); an IOMMU (Intel VT-d or AMD-Vi, Arm SMMUv3, the RISC-V IOMMU) confines a userspace driver's device to its buffers, and on x86_64 its interrupts to its own vector (interrupt remapping; `docs/pci-acpi-smp.md`)
- **Sound** — `/dev/sound`, one `ctl` and `data` per PCM stream of every card (the `sound` core; cards from virtio-sound and USB Audio speakers), mixed by the sound server `sndd` for every program at once; `play` plays a tone or a WAV file, `rec` records one (`docs/sound.md`)
- **Userspace BSD sockets** — libgloss shim over Plan 9 `/net` (no socket syscall); trimmed `curl` HTTPS GET; `AF_UNIX` stream sockets over `/net/unix`, kept in the kernel (`docs/sockets-unix.md`)
- **CI** — GitHub Actions with rust-cache; userspace port outputs are OCI artifacts on GHCR
- **Power** — `poweroff`, `reboot` and `halt` stop the processes, unmount the disks and power off or reset the board: ACPI S5 and the reset register on a PC, PSCI on AArch64, the SBI on RISC-V (`docs/power.md`)
- **Optional: Linux syscall compatibility** — the `linux` kernel module (loaded at boot by the kernel with `--features linux_compat`, else by the boot script's default `rc.local` with `insmod /lib/modules/linux`) runs musl binaries (x86_64, aarch64, riscv64) via `linux PROGRAM` (see `docs/linux-compat.md`)

## Prerequisites

- [rustup](https://rustup.rs/) — `rust-toolchain.toml` pins **nightly-2026-07-26** and installs components automatically
- QEMU (`qemu-system-x86`, `qemu-system-arm`, `qemu-system-riscv` for riscv64 (`qemu-system-misc` before Ubuntu 26.04), `qemu-efi-aarch64`, `qemu-efi-riscv64`); CI runs Ubuntu 26.04's QEMU 10.2; the riscv64 boot has its IOMMU only with QEMU 9.2 or later (`-device riscv-iommu-pci`; 24.04's 8.2 boots without it, and the launcher says so once)
- `clang` + `lld` — cross-compiler and linker for C userspace on every arch (`ld.lld`); clang 19 or later for mlibc (C++23)
- `make` — newlib build
- `meson`, `ninja-build` — mlibc build (`toolchain/mlibc`, `docs/mlibc.md`)
- `cmake` — the C++ runtimes' build (libc++, libc++abi, libunwind against mlibc: `toolchain/libcxx`, `docs/mlibc.md` "C++")
- `git` — fetching upstream port sources
- `gh` — GitHub CLI (CI registry pulls)
- `libc6-dev` — host CRT (`Scrt1.o`) for the Limine host tool
- `rsync` — preparing oksh/sbase build trees
- `patch` — applying `.myos.patch` files
- `curl` — fetching the pinned Limine binary on first build
- `xorriso` — hybrid ISO output (`cargo run -- iso` only)
- `zstd` — compressing the boot archives (`src/initramfs.rs`) and unpacking CI's cached ports (`scripts/ci-registry.sh pull`)
- `e2fsprogs` — the boot test checks the scratch disk's ext2 with `e2fsck`
- `dosfstools`, `mtools` — `cargo test -p fatvol`, and the boot test's `fsck.fat` of the FAT test disk
- `openssh-client`, `python3` — the boot test's host side (`host.sh`): SSH sessions into the guest's dropbear, and the TCP and QEMU-monitor peers

On Ubuntu 26.04 (on 24.04, `qemu-system-misc` instead of `qemu-system-riscv`):

```sh
sudo apt install qemu-system-x86 qemu-system-arm qemu-system-riscv \
  qemu-efi-aarch64 qemu-efi-riscv64 clang lld make meson ninja-build git gh \
  libc6-dev rsync patch curl xorriso zstd e2fsprogs dosfstools mtools \
  openssh-client python3
```

Building every port from source takes hours on the first run. Pulling the
ones CI already built takes about a minute: see "Fast local setup" in
`AGENTS.md`, which also covers sandboxes behind a proxy.

On macOS (Homebrew):

```sh
brew install qemu llvm lld texinfo coreutils meson ninja
# Homebrew's llvm is keg-only (no ld.lld on PATH) — the newlib/ports cross
# wrappers exec ld.lld, so prepend it:
export PATH="/opt/homebrew/opt/llvm/bin:$PATH"   # /usr/local/opt/llvm/bin on Intel
```

Notes:
- `llvm` provides `llvm-strip`/`llvm-ar` used by the cross wrappers; `lld` provides `ld.lld` (the ELF linker — on current Homebrew the `llvm` formula alone no longer includes it); `texinfo` provides `makeinfo` (newlib docs; its `missing` shim only warns, but installing keeps the build log clean); `coreutils` provides `nproc` (also made optional in `toolchain/newlib/build.sh`).
- macOS's stock `/bin/bash` is 3.2 (2007). `brew install bash` with Homebrew's bin first in `PATH` avoids bash-3.2 parser quirks in build scripts.

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
cargo run -- test --skip net,pkg,heavy,install,alpine-disk   # the quick tests (`test-mini`)
cargo run -- uefi test-mini
cargo run -- aarch64 test         # every test: packages, HTTPS, SSH, os-test (needs the network)
```

**Interactive use** — type at `$` prompt (PS/2 keyboard in QEMU window on x86; serial on all arches):

```sh
cargo run          # x86 BIOS — keyboard or serial
cargo run -- uefi  # x86 UEFI
cargo run -- aarch64   # serial only for now
```

`cargo run -- rpi4` writes `target/rpi4.img`, the AArch64 boot disk for a
Raspberry Pi 4 behind its UEFI firmware (serial console, HDMI and USB,
untested on hardware; `docs/raspberry-pi.md`). `cargo run -- rpi3` boots a
Raspberry Pi 3 in QEMU (`raspi3b`: U-Boot, then Limine, a serial shell) and
leaves the boot files for a real one in `target/rpi3`; it also needs
`gcc-aarch64-linux-gnu`, `bison`, `flex`, `bc`, `libssl-dev`,
`libgnutls28-dev` (U-Boot's build) and `device-tree-compiler` (`fdtput`).

The boot test types `root` at `login: ` (the password is empty), then `sh /lib/myos-tests/run.sh mini|full` at `$ `.

## Architecture Overview

```
Boot (Limine)
  └─ Limine HHDM + memmap + framebuffer + modules
       └─ Kernel (higher-half, #![no_std])
            ├─ Heap (linked-list allocator, a quarter of RAM: 64 MiB to 1 GiB)
            ├─ Scheduler (round-robin kernel threads + user tasks)
            ├─ VFS (mount table → rootfs / tmpfs / devfs / procfs / ext2 / netfs)
            ├─ Modules (Limine list, in order): console, hello, pci_enum,
            │     acpi, rpi_firmware, virtio_blk, virtio_scsi, nvme, sdmmc, sound, virtio_sound, usb, xhci, ehci, uhci, ohci, dwc2, usb_hub, usb_storage, usb_hid, usb_audio, usbdev, virtio_net, tap, netfs, fat, ext2
            │     (pcidev and pl061 ship for `insmod`, not loaded at boot)
            └─ Userspace (ELF processes)
                 ├─ /ok smoke (always-on alloc/user/fat/proc markers)
                 ├─ /netd (smoltcp over /dev/net0/data and /dev/tap; only opener of net0)
                 ├─ /etc/rc in the background → /data/etc/rc.local (insmod linux, dropbear :2222)
                 ├─ getty on ttyS0, tty1…ttyN → login → /sh (oksh 7.9 via newlib/libgloss)
                 └─ CI /heap: std / C / sbase / ripgrep / tcc (+ uutils, git: full mode)
```

### Boot
Limine protocol base revision 6 (`limine` crate 0.6.5). Host tool fetches pinned Limine `v12.6.1`, writes the boot disk (GPT, a FAT32 ESP with boot slots `a` and `b`, an ext2 data partition: `docs/install.md`), `limine.conf`, the kernel ELF (its loadable segments only: the debug info and symbols stay in the ELF under `target/`, `boot_kernel` in `src/limine_image.rs`) and the initramfs, which carries the module ELFs (`/lib/modules/<name>`, `src/limine_image.rs` `BOOT_MODULES`): Limine loads those two files and nothing else. On x86, `limine bios-install` makes the image BIOS+UEFI bootable. No `bootloader` crate, no QEMU `-kernel`, no Multiboot.

### Memory
Kernel linked in higher half (`0xffffffff80000000` on x86_64). Limine provides HHDM; usable memory = `phys + HHDM`. Page tables allocated from bump allocator after heap. AArch64 device MMIO (UART, GIC, RTC, PCIe, virtio-mmio) mapped in a window of the higher half (`TTBR1`, `kernel/src/arch/aarch64/paging.rs`); `TTBR0` is user space only.

### Scheduling
Round-robin kernel threads + user tasks across all online CPUs, from per-CPU run queues; a user process can run several threads, which share its address space and run on its home CPU, and an idle CPU takes a user task waiting behind a busy one (`docs/threads.md`) (Limine MP bring-up on x86_64, AArch64 and RISC-V; see `docs/pci-acpi-smp.md`). `task::yield_now()` cooperative; timer IRQ calls `task::schedule()` after EOI → preemptive even in user mode. Blocking waits (`read` on a tty/pipe/pty, `wait`, `nanosleep`, `select`/`poll`) put the task in a `Blocked` state and are woken by the producer (`task::wake`), a deadline or a signal; idle CPUs halt (`hlt`/`wfi`) until an interrupt or a targeted reschedule IPI. `/proc/cpuinfo` shows per-CPU schedule, idle-halt and pull counts. x86_64: LAPIC timer (x2APIC when the CPU has one), TSC for the monotonic clock at the rate CPUID states or calibrated against the PIT. AArch64: generic timer (GICv2 or GICv3 PPIs), `CNTVCT`. RISC-V: `stimecmp`, `time` CSR at the device tree's `timebase-frequency`. All three tick at 100 Hz but program the timer for the earliest sleep deadline when it is sooner (x86_64 by cutting the LAPIC timer's current period short), so `nanosleep` / `poll` timeouts are not rounded up to 10 ms.

### Console & Input
Serial + Limine framebuffer (the `console` module; boot output before it loads is replayed to it). The framebuffer has ten text screens: 0 the console's (the kernel's messages, the boot's), 1–9 the terminals `/dev/tty1`…`tty9`, each keeping its text while another is shown; Alt+N switches. The keyboard (PS/2 on x86 via 8042 probe, virtio-input on the `virt` boards, a USB keyboard on any) types at the shown screen's terminal; the serial port is `/dev/ttyS0` once its getty runs (`docs/tty.md`).

## Documentation

TODO-INDEX

## License

myos's own code is dual-licensed under [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.

A built image also contains third-party programs under their own licenses (some copyleft: Git, Lynx, GNU Make, TinyCC). See [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) for the full list, the source offer and the license texts in [`licenses/`](licenses/); the same files are copied into the root of the ISO built by `cargo run -- iso`.
