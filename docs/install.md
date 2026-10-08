# Installing: the boot disk

The disk images (`target/bios.img`, `target/uefi.img`, `target/aarch64.img`,
`target/riscv64.img`; `write_boot_disk` in `src/limine_disk.rs`) are what an
installed myos looks like: a GPT disk laid out for upgrades that swap the
kernel and the initramfs (issue #351), with room for state that survives
them.

| Partition | Type | Size | myos sees it as (a virtio disk) | Holds |
|-----------|------|------|---------------------------------|-------|
| 1 `BIOS Boot` | BIOS boot | 1 MiB at LBA 2048 | `/dev/vda/p1` | Limine's BIOS stage 2 (`limine bios-install`, x86) |
| 2 `EFI System` | ESP (`c12a7328-…`) | 512 MiB, FAT32 | `/dev/vda/p2` | Limine, the boot slots and `limine.conf` |
| 3 `myos data` | Linux data (`0fc63daf-…`) | 64 MiB, ext2 | `/dev/vda/p3` | state an upgrade keeps; empty for now |

The image is 579 MiB, written sparse: only what the partitions hold takes
space on the host.

## The ESP: two boot slots

```
EFI/BOOT/BOOTX64.EFI          Limine (BOOTAA64.EFI, BOOTRISCV64.EFI)
boot/limine/limine.conf       the only Limine config
boot/limine/limine-bios.sys   x86: Limine's BIOS stage 3
boot/a/kernel                 slot a: the kernel (its loadable segments)
boot/a/initramfs              slot a: the initramfs
boot/b/                       slot b: empty until an upgrade writes it
boot/virt-aarch64.dtb         aarch64, riscv64 (virt.dtb): the device tree
```

Each slot holds a kernel and the initramfs built with it (with the Linux
layer some 50 MiB together), so the ESP has room for both and to grow.
`limine.conf` has an entry per filled slot, the active one first (Limine's
`default_entry: 1`):

```
serial: yes
timeout: 3
default_entry: 1

/myos b
    protocol: limine
    path: boot():/boot/b/kernel
    module_path: boot():/boot/b/initramfs

/myos a
    protocol: limine
    path: boot():/boot/a/kernel
    module_path: boot():/boot/a/initramfs
```

With two entries the menu waits three seconds, so the other slot can be
picked at the console when the active one does not boot; with one (a fresh
image: only slot `a`) it boots at once. aarch64 and riscv64 add their
`global_dtb` (and riscv64 `paging_mode`) lines, as `src/main.rs` writes them.

An upgrade writes the slot that is not active, then rewrites
`boot/limine/limine.conf` with that slot first. That file is the switch, and
the only file the switch touches: Limine reads it from no other place (the
images used to carry copies at `/limine.conf` and `/EFI/BOOT/limine.conf`).
Writing a slot's files leaves the running one, and the config pointing at
it, as they were until the switch.

## The data partition

An empty ext2 (`ext2fs::mkfs`, the same code as myos's `mkfs.ext2`) for what
must outlive an upgrade: SSH host keys and `authorized_keys` (in `/tmp` for
now, `docs/ssh.md`), `/etc` overrides, home directories. myos does not mount
it at boot yet; which of those move there is its own change. On a disk larger
than the image (a VPS), the space after it is left unused: growing the
partition to the disk needs the GPT rewritten, which can come later.

## The first install

From a rescue system (a VPS provider's, a live Linux), write the image
(`cargo build` makes it; `uefi.img` and `bios.img` are the same disk, both
BIOS and UEFI bootable) over the disk:

```sh
zstd -c target/uefi.img | ssh root@rescue 'zstd -d | dd of=/dev/sda bs=1M conv=fsync'
```

(`/dev/sda` is the rescue system's Linux name for the disk. zstd keeps the
transfer to what the image holds; `dd` writes all of it, zeros included, so
nothing of what the disk held before shows through.)

The GPT's backup is at the end of the image, not of the disk: tools that
check it (`sgdisk -e`, `parted`) offer to move it to the end, which is safe.
A VPS without a rescue system but with custom ISOs boots the hybrid ISO
(`cargo run -- iso`), a live medium with the kernel and the initramfs at
`boot/` and none of this layout; installing from it to a disk is the upgrade
tool's `--install` (issue #351).

## Inside the boot tests

The boot tests boot slot `a`. On aarch64 and riscv64 the boot disk is a
virtio disk myos sees, and `kernel.sh`'s `boot_disk` checks its layout: the
slots, the one `limine.conf`, the empty ext2 data partition (x86 boots from
an IDE disk myos has no driver for).
