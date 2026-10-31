# Building, running and real hardware

This page covers what `cargo build` produces, how to boot those images in
QEMU by hand or on a real machine, and the serial console. It ends with
the memory layout. For the launcher's own modes (`cargo run --
[uefi|aarch64|riscv64]`, the tests) see `README.md` and `docs/testing.md`.

## Building on macOS

With Homebrew:

```sh
brew install qemu llvm lld texinfo coreutils meson ninja cmake bash
# Homebrew's llvm is keg-only (no ld.lld on PATH) — the newlib/ports cross
# wrappers exec ld.lld, so prepend it:
export PATH="/opt/homebrew/opt/llvm/bin:$PATH"   # /usr/local/opt/llvm/bin on Intel
```

`lld` is a formula of its own on current Homebrew. Apple's clang has no
RISC-V backend, so the riscv64 builds need Homebrew's llvm first in `PATH`.
macOS's `/bin/bash` is 3.2: Homebrew's bash, with Homebrew's bin first in
`PATH`, avoids its parser quirks in the build scripts.

## The images

| File | What |
|------|------|
| `target/bios.img` | BIOS+UEFI hybrid disk |
| `target/uefi.img` | UEFI-only ESP |
| `target/fat.img` | 20 MiB FAT16 data disk (second virtio-blk) |
| `target/aarch64.img` | AArch64 ESP |
| `target/rpi4.img` | the same for a Raspberry Pi 4 (`cargo run -- rpi4` only) |
| `target/riscv64.img` | RISC-V ESP |

Limine comes from a sha256-pinned release (`ports/limine/versions.env`).
It boots the kernel with protocol base revision 6. The boot disk is GPT
with a FAT32 ESP holding the boot slots `a` and `b`, and an ext2 data
partition (`docs/install.md`). The ESP carries `limine.conf`, the kernel
ELF (loadable segments only; the debug info stays in `target/`) and the
initramfs with the modules as `/lib/modules/<name>`
(`src/limine_image.rs`). On x86, `limine bios-install` makes the image
boot from BIOS as well. There is no QEMU `-kernel` and no Multiboot.

## Running in QEMU

`cargo run` boots the images itself. To boot them by hand:

```sh
# x86_64 BIOS (UEFI: the same with target/uefi.img)
qemu-system-x86_64 -m 256 \
  -drive format=raw,file=target/bios.img \
  -drive if=none,id=vd0,format=raw,file=target/fat.img \
  -device virtio-blk-pci,drive=vd0 \
  -serial stdio

# AArch64
qemu-system-aarch64 -m 256 -cpu cortex-a72 -machine virt,gic-version=2 \
  -drive format=raw,file=target/aarch64.img \
  -drive if=none,id=vd0,format=raw,file=target/fat.img \
  -device virtio-blk-device,drive=vd0 \
  -bios /usr/share/AAVMF/AAVMF_CODE.fd -serial stdio

# RISC-V
qemu-system-riscv64 -m 256 -machine virt,aia=aplic-imsic \
  -drive format=raw,file=target/riscv64.img \
  -bios /usr/share/qemu/ovmf-bin/Edk2-riscv64.fd -serial stdio

# x86_64 hybrid ISO
cargo run -- iso          # writes target/myos-x86_64.iso
qemu-system-x86_64 -m 256 -cdrom target/myos-x86_64.iso -serial stdio
```

The interrupt controllers must match the device tree the launcher packed
into the image:

- AArch64: `MYOS_AARCH64_GIC=3 cargo run -- aarch64` builds for a GICv3
  board, which also runs without ACPI (`acpi=off` in `-machine`).
- RISC-V: the default is the AIA. `MYOS_RISCV_INTC=plic cargo run --
  riscv64` builds for the PLIC (plain `-machine virt`).

`poweroff` exits QEMU on every arch (`docs/power.md`). A kernel panic
exits it too (`isa-debug-exit` on x86_64, PSCI or SBI shutdown
elsewhere).

## Disks

Every QEMU run attaches `target/fat.img` as the second virtio-blk disk.
It holds `/msg`, which the `[ OK ] fat` and msg boot markers check. A
machine without it still boots to the shell.

Every `cargo run` rebuilds the test disks, so nothing the guest writes
survives. For a long-lived system in QEMU (interactive runs, not `test`),
use a persistent disk:

```sh
MYOS_DISK=~/myos-disk.img cargo run          # attach a persistent extra disk (created sparse, 8 GiB, if missing)
# in the guest: cat /dev/*/info to find it, then
get-myos --install --local <disk>            # lay the boot disk out on it (docs/install.md)
# power off, then boot that disk alone from now on:
MYOS_BOOT_DISK=~/myos-disk.img cargo run     # also: uefi, aarch64, riscv64
```

An installed disk keeps `/data` across upgrades (`get-myos --upgrade`);
a newer `cargo build` does not touch it.
`MYOS_DISK` can stay set beside `MYOS_BOOT_DISK` to attach a second data
disk, if the two paths differ (QEMU locks images).

## Real hardware

Write `target/uefi.img` or `target/bios.img` (the same disk, BIOS and
UEFI bootable) to a USB stick or an internal drive. It has the installed
layout of `docs/install.md`: a 512 MiB FAT32 ESP with two boot slots and
an ext2 data partition. The screen shows tty1's login; Alt+1…9 switch
ttys and Alt+0 shows the console (`docs/tty.md`).

On macOS, find the stick's disk in `diskutil list` (`disk4` below) and
write the whole disk, not a partition; the raw device (`/dev/rdisk4`) is
much faster:

```sh
diskutil list
disk=disk4
diskutil unmountDisk /dev/"$disk"
sudo dd if=target/bios.img of=/dev/r"$disk" bs=4m
sync
diskutil eject /dev/"$disk"
```

On Linux, `sudo dd if=target/bios.img of=/dev/sdX bs=4M conv=fsync`
does the same.

Upgrades, installs onto another disk and VPS setups are in
`docs/install.md`. For SSH, bring your key (`docs/ssh.md`): dropbear
listens on port 2222.

## Console input and serial ports

stdin merges the keyboard and the serial port. The keyboard goes through
a loadable keymap: `user/init` loads Swiss German (`/lib/kbd/ch.map`),
with US as the fallback (`docs/keymap.md`). The serial port needs no
map.

| Arch | Serial port | Baud rate |
|------|-------------|-----------|
| x86_64 | COM1 (I/O `0x3F8`) | 38400 8N1 |
| AArch64 | PL011 UART | 115200 8N1 |
| RISC-V | UART | 115200 8N1 |

## Memory layout

- The kernel runs in the higher half (`0xffffffff80000000` on x86_64).
  Limine maps physical memory at `phys + HHDM`.
- On AArch64 the kernel's mappings are in `TTBR1` and user space alone
  in `TTBR0`. Device MMIO has its own higher-half window at
  `0xffff_8000_0000_0000 + phys`, because Limine's HHDM leaves devices
  out (`kernel/src/arch/aarch64/paging.rs`).
- User space starts at VA 0: `[0, 512 GiB)` on x86_64, `[0, 66 GiB)` on
  aarch64 and riscv64. A position-independent image loads at 1 GiB, an
  `ET_EXEC` one at its link address. The stack, heap and the 64 GiB mmap
  window lie above the image.
- Modules run from the kernel heap (rwx, in the HHDM).
