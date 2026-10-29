# The source tree

This page maps the myos tree by area, one row per directory or group of
related files, with the page in `docs/` that covers each. It adds to the
short table in `AGENTS.md`: use it to find where a feature lives, then
read the directory itself. Build output and fetched sources all land in
`target/`, which is never committed.

## Host side

| Path | What |
|------|------|
| `src/main.rs` | The launcher: QEMU per arch and machine, the test modes, `MYOS_DISK` / `MYOS_BOOT_DISK` (`docs/running.md`) |
| `src/limine_*.rs`, `src/initramfs.rs` | The images: Limine, the boot disk (GPT, FAT32 ESP, ext2), the FAT16 data disk, the initramfs packer (`docs/running.md`, `docs/ports.md`) |
| `src/ports.rs`, `src/packages.rs`, `src/release.rs` | The port descriptors, the package tarballs and indexes, the release a build belongs to (`docs/ports.md`, `docs/packages.md`) |
| `src/boot_test.rs`, `src/test_www.rs`, `src/sound.rs`, `src/usb2.rs`, `src/wifi_*.rs` | The boot test's host side and the peers it gives the guest: the test web server, sound and USB devices, the simulated Wi-Fi network (`docs/testing.md`) |
| `build.rs` | Builds the x86_64 kernel and the images; runs missing ports' build scripts |
| `etc/` | The image's `/etc`: the security policy (`docs/security.md`), the boot script `rc`, the default `rc.local` (`docs/install.md`), `ttys`, `hosts`, `services` |
| `boot/rpi3` | The Raspberry Pi 3's ARM stub and U-Boot build (`docs/raspberry-pi.md`) |
| `targets/` | Custom Rust target specs (`<arch>-unknown-myos`) for the `std` programs |
| `scripts/` | `ports.sh` (reads the descriptors), the CI registry and planner, the ports' hash and freshness functions (`myos-c-userspace-lib.sh`) |

## Kernel

The kernel is under `kernel/src/`. `main.rs` is the Limine entry: it sets
up the heap, interrupts, the scheduler and the root filesystem, then
starts the modules and `init`.

| Path | What |
|------|------|
| `arch/{x86,aarch64,riscv64}` | All per-arch code: boot, UART, traps and interrupts, paging, context switch, FPU, clock, SMP glue |
| `platform.rs`, `acpi.rs`, `dt.rs` | The board description from ACPI and the device tree (`docs/platform.md`) |
| `mm.rs`, `heap.rs` | Physical frames and the kernel heap |
| `irq.rs`, `irqchip.rs`, `msi.rs` | Interrupts (`docs/interrupts.md`) |
| `pci.rs`, `dma.rs`, `iommu/` | PCI, DMA and the IOMMUs (`docs/pci.md`, `docs/iommu.md`) |
| `task/` | Tasks, processes, threads, the scheduler, process groups, namespaces (`docs/scheduler.md`, `docs/threads.md`) |
| `user/` | User address spaces, program loading, the syscalls (`user/syscall.rs`, `docs/userspace.md`) |
| `fs/` | The VFS and its in-kernel filesystems, the page cache, the initramfs reader (`docs/fs.md`) |
| `blk.rs`, `blk/` | Block devices, the block cache, GPT |
| `console.rs`, `input.rs`, `tty.rs`, `pty.rs` | The console, the ttys and ptys (`docs/tty.md`) |
| `signal.rs`, `evfd.rs`, `pipe.rs` | Signals (`docs/signals.md`), event fds (`docs/events.md`), pipes |
| `sec/` | The security policy (`docs/security.md`) |
| `modules/`, `personality.rs` | The module loader and its `KernelApi`, foreign syscall personalities (`docs/modules.md`) |
| `power.rs`, `rng.rs`, `time.rs`, `smp.rs` | Power-off and reboot (`docs/power.md`), the CSPRNG, the RTC, multi-core bring-up |

## Modules

Loadable kernel modules are under `modules/`, one directory each
(`docs/modules.md`). `modules/abi` holds their shared `#[repr(C)]`
interface and `ABI_VERSION`.

| Area | Modules |
|------|---------|
| Platform and buses | `acpi` (`docs/platform.md`, "AML"), `pci_enum`, `pcidev` (`docs/pci.md`), `pl061` (`docs/interrupts.md`), `rpi_firmware` (`docs/raspberry-pi.md`) |
| virtio | `virtio`, `virtq` (shared code), `virtio_blk`, `virtio_scsi`, `virtio_net`, `virtio_sound` |
| Storage and filesystems | `nvme`, `sdmmc` (SD and SDIO), `fat`, `ext2` (each over a host-tested crate) |
| USB | `usb` (the core), `xhci`, `ehci`, `uhci`, `ohci`, `dwc2`, `usb_hub`, `usb_storage`, `usb_hid`, `usb_audio`, `usbdev` (`docs/usb.md`) |
| Console and sound | `console` (screen, keyboards, mouse, keymaps; `docs/tty.md`, `docs/fb.md`), `sound` (`docs/sound.md`) |
| Network | `netfs` (`/net`, `docs/net.md`), `tap` (`docs/wifi.md`) |
| Other | `linux` (`docs/linux-compat.md`), `hello` (a sample) |

## Native userspace

The programs under `user/`, each with a `port.env` that says what it puts
in the image (`docs/ports.md`; `docs/userspace.md`).

| Path | What |
|------|------|
| `user/init` | PID 1: mounts, starts netd, sndd and `/etc/rc`, and a getty per tty |
| `user/lib` | `myos_user`, the `no_std` programs' syscall library |
| `user/netd`, `user/net`, `user/tls`, `user/dns`, `user/http`, `user/ping` | The network stack (smoltcp in userspace) and its clients (`docs/net.md`, `docs/https.md`) |
| `user/wifi` | The Wi-Fi daemon and its radio crates (`docs/wifi.md`) |
| `user/sndd` | The sound server (`docs/sound.md`) |
| `user/get-myos` | `get-myos` and `run-myos`, the package manager and installer (`docs/packages.md`, `docs/install.md`) |
| `user/insmod`, `rmmod`, `mount`, `umount`, `mkfs.*`, `power`, `ps`, ... | Small system commands |
| `user/c`, `user/c-mlibc`, `user/std`, `user/ok`, `user/heap` | C programs and smokes (newlib and mlibc, `docs/mlibc.md`), the Rust `std` demos, the boot smokes |
| `user/tests` | The boot tests' guest side: the runner and `kernel.sh` (`docs/testing.md`) |

## Toolchains, ports and packages

| Path | What |
|------|------|
| `toolchain/newlib`, `toolchain/mlibc`, `toolchain/libmyos` | The two C libraries and the C they share (`docs/mlibc.md`) |
| `toolchain/libcxx` | LLVM's C++ runtimes against mlibc (`docs/mlibc.md`, "C++") |
| `toolchain/std` | The Rust `std` port and its sysroot |
| `ports/` | Programs in the image, fetched and cross-built (`docs/ports.md`) |
| `packages/` | Programs built and published but not in the image (`docs/packages.md`) |
| `linux-compat/` | The Linux layer's userspace (`docs/linux-compat.md`) |
