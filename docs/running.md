# Building, running and real hardware

## Building and running in QEMU

```sh
cargo build
```

Produces:
- `target/bios.img` — BIOS+UEFI hybrid disk
- `target/uefi.img` — UEFI-only ESP
- `target/fat.img` — 20 MiB FAT16 data disk (second virtio-blk)
- `target/aarch64.img` — AArch64 ESP
- `target/rpi4.img` — the same for a Raspberry Pi 4 (`cargo run -- rpi4` only)
- `target/riscv64.img` — RISC-V ESP

### x86_64 BIOS
```sh
qemu-system-x86_64 -m 256 \
  -drive format=raw,file=target/bios.img \
  -drive if=none,id=vd0,format=raw,file=target/fat.img \
  -device virtio-blk-pci,drive=vd0 \
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
image (`MYOS_AARCH64_GIC=3 cargo run -- aarch64` for a GICv3 board, which
also runs without ACPI tables: add `acpi=off` to `-machine`).

### RISC-V
```sh
qemu-system-riscv64 -m 256 -machine virt,aia=aplic-imsic \
  -drive format=raw,file=target/riscv64.img \
  -bios /usr/share/qemu/ovmf-bin/Edk2-riscv64.fd -serial stdio
```
The interrupt controllers must match the device tree the launcher packed
into the image: the AIA by default, the PLIC (plain `-machine virt`) after
`MYOS_RISCV_INTC=plic cargo run -- riscv64`.

### ISO (hybrid)
```sh
cargo run -- iso          # writes target/myos-x86_64.iso
qemu-system-x86_64 -m 256 -cdrom target/myos-x86_64.iso -serial stdio
```

### Second disk (`target/fat.img`)
Attached as second virtio-blk in all QEMU runs. Holds `/msg` for `[ OK ] fat` / msg markers. Bare metal without it still boots to shell.

### A persistent disk

Every `cargo run` boots a freshly built image and rewrites the test disks, so nothing the guest writes survives. To test over a long time on one disk (QEMU only; interactive runs, not `test`):

```sh
MYOS_DISK=~/myos-disk.img cargo run          # attach a persistent extra disk (created sparse, 8 GiB, if missing)
# in the guest: cat /dev/*/info to find it, then
get-myos --install --local <disk>            # lay the boot disk out on it (docs/install.md)
# power off, then boot that disk alone from now on:
MYOS_BOOT_DISK=~/myos-disk.img cargo run     # also: uefi, aarch64, riscv64
```

An installed disk keeps `/data` (apps, the Alpine root) and upgrades itself with `get-myos --upgrade`; a newer `cargo build` does not touch it. `MYOS_DISK` can stay set while booting `MYOS_BOOT_DISK` from another file to attach a second data disk. QEMU locks images, so the two paths must differ.

## Real Hardware

Write the disk image to a USB stick or an internal drive (`target/uefi.img`; `target/bios.img` is the same disk, both BIOS and UEFI bootable). It is the installed layout (`docs/install.md`): a 512 MiB FAT32 ESP with two boot slots for upgrades (`boot/a/`, `boot/b/`) and the one `limine.conf` that picks between them, then an ext2 data partition for state that outlives an upgrade. The screen shows the boot, then tty1's login; Alt+1…9 switch between the ttys' text screens, Alt+0 shows the console's with the boot's and the kernel's lines (`docs/tty.md`). A running system upgrades itself from the release with `get-myos --upgrade`: the new kernel and initramfs go into the other slot, which boots next, the running one stays in the boot menu as the fallback; `get-myos --install DISK` lays the boot disk out on another disk, from the mirror or, with `--local` and no network, from the running system (the ISO included) (`docs/install.md`). A VPS takes the image written over its disk from the provider's rescue system (`dd`, `docs/install.md`), or, as a live medium, the release's hybrid ISO (`curl -L .../myos-x86_64.iso | dd of=/dev/sda`); to reach it over SSH (dropbear on port 2222, started by the boot script `/data/etc/rc.local`, which the administrator edits: `docs/install.md`), bring your own key (`docs/ssh.md`).

**stdin** merges keyboard and serial. Keyboard characters come from a **loadable keymap** (default Swiss German via `user/init` → `/lib/kbd/ch.map`, US fallback; see `docs/keymap.md`). Serial always available and does not need a map.

| Arch | Serial port | Baud rate |
|------|-------------|-----------|
| x86_64 | COM1 (I/O `0x3F8`) | 38400 8N1 |
| AArch64 | PL011 UART | 115200 8N1 |
| RISC-V | UART | 115200 8N1 |

## Memory and boot notes

- `poweroff` exits QEMU on every arch (`docs/power.md`); the boot test ends with it. A kernel panic exits too: `isa-debug-exit` on x86_64, PSCI `SYSTEM_OFF` / SBI shutdown elsewhere
- Kernel linked in higher half; Limine sets stack, enables MMU, provides HHDM
- User address space from VA 0: `[0, 512 GiB)` (PML4[0]) on x86_64, `[0, 66 GiB)` on aarch64 and riscv64; a position-independent image loads at 1 GiB, an `ET_EXEC` one at its link address (above 64 KiB), with its stack, heap and the 64 GiB mmap window above it. riscv64 maps device memory below 66 GiB at 128 GiB + phys (`RELOC_VA`)
- AArch64 device MMIO in a higher-half window (`0xffff_8000_0000_0000 + phys`, 1 GiB device blocks, physical addresses below 127.5 TiB, in at most 16 regions of 512 GiB that hold devices; not in the HHDM at Limine base rev 3+)
- Modules run from HHDM heap (rwx); loader flushes I-cache on AArch64 after copy
- Limine binaries downloaded from GitHub release `v12.6.1` (sha256-pinned) into `target/limine-v12.6.1`
- `user/ok` exits early if `/msg` absent (FAT disk not present); CI fails if `[ OK ] msg` / fat markers are missing
