# Installing: the boot disk

The disk images (`target/bios.img`, `target/uefi.img`,
`target/aarch64.img`, `target/riscv64.img`; `src/limine_disk.rs`) are
what an installed myos looks like. Each is a GPT disk laid out for
upgrades that swap the kernel and the initramfs (issue #351), with room
for state that survives them. This page covers the layout, upgrading,
installing on another disk, and what is mounted and run at boot.

| Partition | Type | Size | myos sees it as (a virtio disk) | Holds |
|-----------|------|------|---------------------------------|-------|
| 1 `BIOS Boot` | BIOS boot | 1 MiB | `/dev/vda/p1` | Limine's BIOS stage 2 (x86) |
| 2 `EFI System` | ESP | 512 MiB, FAT32 | `/dev/vda/p2`, mounted at `/boot` | Limine, the boot slots, `limine.conf` and `fstab` |
| 3 `myos data` | Linux data | 64 MiB (the rest of the disk after `--install`), ext2 | `/dev/vda/p3`, mounted at `/data` | state an upgrade keeps: `get-myos`'s apps, `get-alpine`'s root |

The image is 579 MiB, written sparse. A disk behind virtio-scsi (a
Hetzner VPS) shows up as `/dev/sda` instead (`modules/virtio_scsi`); the
fstab names partitions by GUID, so this does not matter.

## The ESP: two boot slots

```
EFI/BOOT/BOOTX64.EFI          Limine (BOOTAA64.EFI, BOOTRISCV64.EFI)
boot/limine/limine.conf       the only Limine config
boot/limine/limine-bios.sys   x86: Limine's BIOS stage 3
boot/a/kernel                 slot a: the kernel
boot/a/initramfs              slot a: the initramfs (zstd, issue #621)
boot/a/version                slot a: its release, the initramfs's /lib/myos-release
boot/b/                       slot b: empty until an upgrade writes it
boot/virt-aarch64.dtb         aarch64, riscv64 (virt.dtb): the device tree
boot/tests                    the build's disks only: the tests module
fstab                         what `mount -a` mounts at boot (below)
```

A slot takes some 6 MiB, so the ESP has room for both and to grow. The
build's disks also carry the tests module (`docs/testing.md`), which
their entries load and an upgrade keeps.

`limine.conf` (`src/limine_image.rs`) has an entry per filled slot, the
active one first:

```
serial: yes
timeout: 3
default_entry: 1

/myos b
    protocol: limine
    path: boot():/boot/b/kernel
    module_path: boot():/boot/b/initramfs
    cmdline: slot=b

/myos a
    ...
    cmdline: slot=a
```

- With two entries the menu waits three seconds, so the other slot can
  be picked at the console when the active one does not boot. With one
  (a fresh image) it boots at once.
- `cmdline: slot=X` tells the running system which slot it came from
  (`/proc/cmdline`), including a fallback picked by hand.
- An upgrade writes the slot that is not running, then rewrites
  `limine.conf` with that slot first. That one file is the switch.

## Upgrading: `get-myos --upgrade`

```sh
get-myos --upgrade        # [-m MIRROR] [-f]
```

Run it as root on the running system (`user/get-myos/boot.c`):

1. Find the boot disk: the ESP whose `boot/<slot>/version` matches
   `/lib/myos-release`, `<slot>` from `/proc/cmdline`. It is normally
   mounted at `/boot`; otherwise get-myos mounts it for the duration.
2. Read the mirror's `<arch>-boot.txt` (`docs/packages.md`): the
   release's files with their sizes and SHA-256. Nothing to do when it
   is not newer than the running slot; `-f` writes anyway.
3. Write the other slot. Each file is checked in memory before it is
   written and read back after a remount. The slot's `version` is
   written last, so a half-written slot has none.
4. Update Limine's files when the release's differ: write each beside
   the old one, read them back, rename over. On x86 a new
   `limine-bios.sys` reruns `limine bios-install`, since the BIOS stage
   must match it.
5. Switch: rewrite `limine.conf` with the new slot first and the running
   one as fallback. When step 4 changed Limine, the config comes from the
   release, and lines edited by hand are lost.

get-myos does not reboot; `reboot` starts the new slot. A failure before
step 4 leaves the running system as it was. Step 4 is the one step that
is not atomic (one rename after another, `bios-install` writes in place),
but it runs only on a new Limine pin.

The release's initramfs is the default build's, without the Linux layer;
an image built with `--features linux_compat` brings it back.

## Installing on another disk: `get-myos --install DISK`

```sh
get-myos --install nvme1n1    # or /dev/nvme1n1, /dev/nvme1n1/data
```

This erases DISK and lays out the same GPT, with the data partition over
the rest of the disk. It formats the ESP (FAT) and the data partition
(ext2), writes Limine's files and slot `a` from the mirror, checked as in
an upgrade, and on x86 runs `limine bios-install` (the `limine` port, at
`/bin/etc/limine`). The disk then boots by BIOS and by UEFI.

It refuses the running boot disk, a disk with a partition mounted, and a
disk under 580 MiB. Nothing comes from the running ESP, so a system
booted from the ISO (`cargo run -- iso`) installs the same way.

### Without a network: `--install DISK --local`

```sh
get-myos --install nvme1n1 --local
```

This installs the running system itself:

- The kernel and the initramfs come from this boot: the kernel keeps
  what Limine loaded at `/proc/boot/kernel` and `/proc/boot/initramfs`,
  however it booted (ISO, USB stick, disk).
- Limine's files come from the initramfs, at `/lib/myos-boot/` with a
  `boot.txt` in the release list's format.
- Slot `a` gets this system's release. From the ISO that includes the
  Linux layer; a later `--upgrade` from a mirror drops it.
- A system booted with the tests module (`/proc/boot/tests`) installs it
  too, with a `limine.conf` that loads it.

get-myos lists them in `.get-myos/local-boot` (`docs/packages.md`) and
installs from that list as from a mirror's.

## Mounted at boot: `/boot`, `/data` and the fstab

init runs `mount -a` (`user/mount`) before anything else:

1. The ESP the system booted from is mounted at `/boot`. Limine tells the
   kernel which partition it loaded from: `/proc/boot/partuuid`. Booted
   from the ISO there is none, and nothing below happens. `mount -a`
   waits up to 10 seconds for that partition, since a USB stick's disk
   appears only once the USB driver has found it.
2. `/tmp/mnt` is bound over `/mnt`, since the root (the initramfs) is
   read-only. A disk at `/mnt/disk` shows as `/tmp/mnt/disk` in
   `/proc/mounts`.
3. Each line of `/boot/fstab` is mounted unless its partition already is:

```
# What `mount -a` mounts at boot (docs/install.md): PARTUUID=<guid> MOUNTPOINT FSTYPE [rw],
# the partition's unique GUID from /proc/partitions; a mount point under /mnt is made.
PARTUUID=6d796f73-0000-4000-8000-000000000004 /data ext2
```

A partition is named only by its unique GUID, because a disk's name
(`nvme0n1`, `vda`) depends on the machine. A line for a missing
partition is skipped. A wrong line (a `/dev/` path, an option other than
`rw`, as there are no read-only mounts) or one that fails to mount is
reported, but does not stop the boot. The mount point must exist or be
under `/mnt`.

The image builder and `get-myos --install` write the fstab; `--upgrade`
leaves it alone. The images' GUIDs are fixed, so of two disks written
from the same image, the first found is mounted.

To mount a disk at every boot:

```sh
grep nvme1n1/ /proc/partitions      # the fifth field: the partition's unique GUID
echo 'PARTUUID=<that guid> /mnt/disk ext2' >> /boot/fstab
mount -a                            # mounts what is not mounted yet
```

`mount PARTUUID=<guid> DIR FSTYPE` mounts one by hand. A filesystem on a
whole disk or in an MBR partition has no partition GUID and cannot be
listed.

## The boot script: `/etc/rc` and `/data/etc/rc.local`

After `mount -a` and netd, init starts `/etc/rc` (`etc/rc`) in the
background and goes on to the logins. The script writes to the console's
screen (`Alt+0`, `docs/tty.md`). Its last line is also written to
`/tmp/boot.done`:

```
[ OK ] boot complete (/data/etc/rc.local)
[ WARN ] boot complete (/data/etc/rc.local exited 1)
```

The policy puts the script in the `rc` domain, whose only power is to
become root in `admin` (`sec as root`, `docs/security.md`). It then:

1. binds a writable directory over `/etc/dropbear` for the host keys and
   `authorized_keys` (`docs/ssh.md`): `/data/dropbear` when `/data` is
   mounted, else `/tmp/dropbear`, both `secret(root)`;
   and the same way `/data/home` (else `/tmp/home`, gone at the next
   boot) over `/home`, the users' homes, with a directory made in it for
   each user whose `home:` is below it (`docs/security.md`; root's is
   `/home/root`). A login before that finds the image's `/home/root`,
   empty and read-only;
2. binds the global apps of `/data/apps`
   (`get-myos -r /data/apps --global`, `docs/packages.md`) so they are on
   `PATH` before the first login; log in `/tmp/get-myos-global.log`;
3. when `/data/apps/apps.list` exists, runs `get-myos -r /data/apps
   --sync` in the background; log in `/tmp/get-myos-sync.log`;
4. copies the image's `/etc/rc.local` to `/data/etc/rc.local` when that
   is missing, and runs it with `sh` (the image's own without `/data`).

`/data/etc/rc.local` is the administrator's to edit; deleting it brings
the default back at the next boot. The default loads the Linux layer
(`docs/linux-compat.md`) and starts dropbear:

```sh
grep -q '^linux$' /proc/modules || insmod /lib/modules/linux
dropbear -R -D /etc/dropbear -p 2222
```

A command left in the foreground holds back the boot's last line (not
the login): start daemons with `&` or their own `-d`.

## The data partition

An ext2 at `/data` for what must outlive an upgrade. Only the
administrator may write it (the policy's `sys.file` label). It holds
`get-myos`'s apps (`/data/apps`) and `get-alpine`'s root
(`/data/alpine`, `docs/linux-compat.md`), both labelled `sys.pkg`, plus
`/data/etc/rc.local`, `/data/dropbear` and the homes, `/data/home`.

On a disk larger than the image, the space after it stays unused:
growing the partition needs the GPT rewritten and the ext2 grown
(issue #488). So a disk written from the image has 64 MiB of `/data`,
enough for apps but not an Alpine root (apk stops with
`No space left on device`). Install with `get-myos --install` from the
ISO to give `/data` the whole disk.

## The first install

From a rescue system (a VPS provider's, a live Linux), write the image
over the disk. `uefi.img` and `bios.img` are the same disk, bootable both
ways.

```sh
zstd -c target/uefi.img | ssh root@rescue 'zstd -d | dd of=/dev/sda bs=1M conv=fsync'
```

The GPT's backup is at the end of the image, not of the disk; tools that
offer to move it (`sgdisk -e`, `parted`) can safely do so.

Without a rescue system, boot the hybrid ISO (`cargo run -- iso`) and
install from it with `get-myos --install` (`--local` offline).

## Testing

The boot tests boot a copy of the image (`target/boot-test-<name>.img`;
on x86 the boot disk is `vdc`). `user/tests/kernel.sh` checks the
layout, the boot script and the fstab. `user/get-myos/test.sh` upgrades
(replacing a stale Limine) and installs, and the launcher then boots the
upgraded disk (`run.sh reboot`) and the installed one
(`run.sh installed`). The ISO is not booted in CI: an install from it
runs the same code.

## History

- The images used to carry copies of `limine.conf` at `/limine.conf` and
  `/EFI/BOOT/limine.conf`. Only `boot/limine/limine.conf` remains, so the
  upgrade's switch is one file.
