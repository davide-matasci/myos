# Building, running and real hardware

This page covers what `cargo build` produces, how to boot those images in
QEMU by hand or on a real machine, and the console's serial ports. It ends
with the memory layout the kernel and the programs run in. For the
launcher's own modes (`cargo run -- [uefi|aarch64|riscv64]`, the tests)
see `README.md` and `docs/testing.md`.

## The images

```sh
cargo build
```

produces:

| File | What |
|------|------|
| `target/bios.img` | BIOS+UEFI hybrid disk |
| `target/uefi.img` | UEFI-only ESP |
| `target/fat.img` | 20 MiB FAT16 data disk (second virtio-blk) |
| `target/aarch64.img` | AArch64 ESP |
| `target/rpi4.img` | the same for a Raspberry Pi 4 (`cargo run -- rpi4` only) |
| `target/riscv64.img` | RISC-V ESP |

The Limine binaries come from the GitHub release `v12.6.1`, sha256-pinned
in `ports/limine/versions.env`, and land in `target/limine-v12.6.1`.

## Running in QEMU

`cargo run` builds the images and boots them in QEMU itself. These
commands boot the built images by hand.

### x86_64 BIOS

```sh
qemu-system-x86_64 -m 256 \
  -drive format=raw,file=target/bios.img \
  -drive if=none,id=vd0,format=raw,file=target/fat.img \
  -device virtio-blk-pci,drive=vd0 \
  -serial stdio
```

### x86_64 UEFI

The same command, with `target/uefi.img`.

### AArch64

```sh
qemu-system-aarch64 -m 256 -cpu cortex-a72 -machine virt,gic-version=2 \
  -drive format=raw,file=target/aarch64.img \
  -drive if=none,id=vd0,format=raw,file=target/fat.img \
  -device virtio-blk-device,drive=vd0 \
  -bios /usr/share/AAVMF/AAVMF_CODE.fd -serial stdio
```

The GIC version must match the device tree the launcher packed into the
image. `MYOS_AARCH64_GIC=3 cargo run -- aarch64` builds for a GICv3 board.
That board also runs without ACPI tables: add `acpi=off` to `-machine`.

### RISC-V

```sh
qemu-system-riscv64 -m 256 -machine virt,aia=aplic-imsic \
  -drive format=raw,file=target/riscv64.img \
  -bios /usr/share/qemu/ovmf-bin/Edk2-riscv64.fd -serial stdio
```

The interrupt controllers must match the device tree the launcher packed
into the image. The default is the AIA.
`MYOS_RISCV_INTC=plic cargo run -- riscv64` builds for the PLIC (plain
`-machine virt`).

### ISO (hybrid)

```sh
cargo run -- iso          # writes target/myos-x86_64.iso
qemu-system-x86_64 -m 256 -cdrom target/myos-x86_64.iso -serial stdio
```

### Power-off and panics

`poweroff` exits QEMU on every arch (`docs/power.md`), and the boot test
ends with it. A kernel panic exits QEMU too: through `isa-debug-exit` on
x86_64, and PSCI `SYSTEM_OFF` / SBI shutdown elsewhere.

## Disks

### The data disk (`target/fat.img`)

All QEMU runs attach `target/fat.img` as the second virtio-blk disk. It
holds `/msg`, which the `[ OK ] fat` and msg markers check.

- `user/ok` exits early if `/msg` is absent (no FAT disk).
- CI fails if the `[ OK ] msg` or fat markers are missing.
- Bare metal without the disk still boots to the shell.

### A persistent disk

Every `cargo run` boots a freshly built image and rewrites the test disks,
so nothing the guest writes survives. To test over a long time on one
disk, use a persistent disk. This works in QEMU only, for interactive
runs, not for `test`.

```sh
MYOS_DISK=~/myos-disk.img cargo run          # attach a persistent extra disk (created sparse, 8 GiB, if missing)
# in the guest: cat /dev/*/info to find it, then
get-myos --install --local <disk>            # lay the boot disk out on it (docs/install.md)
# power off, then boot that disk alone from now on:
MYOS_BOOT_DISK=~/myos-disk.img cargo run     # also: uefi, aarch64, riscv64
```

- An installed disk keeps `/data` (apps, the Alpine root) and upgrades
  itself with `get-myos --upgrade`. A newer `cargo build` does not touch
  it.
- `MYOS_DISK` can stay set while you boot `MYOS_BOOT_DISK` from another
  file, to attach a second data disk. QEMU locks images, so the two paths
  must differ.

## Real hardware

Write the disk image to a USB stick or an internal drive:
`target/uefi.img`, or `target/bios.img`, which is the same disk, both BIOS
and UEFI bootable.

The image has the installed layout (`docs/install.md`):

- a 512 MiB FAT32 ESP with two boot slots for upgrades (`boot/a/`,
  `boot/b/`) and the one `limine.conf` that picks between them;
- then an ext2 data partition, for state that outlives an upgrade.

The screen shows the boot, then tty1's login. Alt+1…9 switch between the
ttys' text screens. Alt+0 shows the console's screen, with the boot's and
the kernel's lines (`docs/tty.md`).

Upgrading and installing (`docs/install.md`):

- A running system upgrades itself from the release with
  `get-myos --upgrade`. The new kernel and initramfs go into the other
  slot, which boots next. The running one stays in the boot menu as the
  fallback.
- `get-myos --install DISK` lays the boot disk out on another disk, from
  the mirror. With `--local` and no network it installs from the running
  system (the ISO included).

On a VPS:

- Write the image over its disk from the provider's rescue system (`dd`,
  `docs/install.md`).
- Or, as a live medium, write the release's hybrid ISO
  (`curl -L .../myos-x86_64.iso | dd of=/dev/sda`).
- To reach it over SSH, bring your own key (`docs/ssh.md`). dropbear
  listens on port 2222. The boot script `/data/etc/rc.local` starts it;
  the administrator edits that script (`docs/install.md`).

## Console input and serial ports

**stdin** merges the keyboard and the serial port.

- Keyboard characters go through a **loadable keymap**. The default is
  Swiss German: `user/init` loads `/lib/kbd/ch.map`, with US as the
  fallback (`docs/keymap.md`).
- The serial port is always available and needs no map.

| Arch | Serial port | Baud rate |
|------|-------------|-----------|
| x86_64 | COM1 (I/O `0x3F8`) | 38400 8N1 |
| AArch64 | PL011 UART | 115200 8N1 |
| RISC-V | UART | 115200 8N1 |

## Memory layout

- The kernel is linked in the higher half. Limine sets up its stack,
  enables the MMU and provides the HHDM.
- The user address space starts at VA 0: `[0, 512 GiB)` (PML4[0]) on
  x86_64, `[0, 66 GiB)` on aarch64 and riscv64.
  - A position-independent image loads at 1 GiB, an `ET_EXEC` one at its
    link address (above 64 KiB).
  - Its stack, heap and the 64 GiB mmap window lie above the image.
  - riscv64 maps device memory below 66 GiB at 128 GiB + phys
    (`RELOC_VA`).
- AArch64 maps device MMIO in a higher-half window:
  `0xffff_8000_0000_0000 + phys`, in 1 GiB device blocks, for physical
  addresses below 127.5 TiB, in at most 16 regions of 512 GiB that hold
  devices. Device memory is not in the HHDM at Limine base revision 3 and
  later.
- Modules run from the HHDM heap (rwx). On AArch64 the loader flushes the
  I-cache after the copy.
