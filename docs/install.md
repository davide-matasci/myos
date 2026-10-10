# Installing: the boot disk

The disk images (`target/bios.img`, `target/uefi.img`, `target/aarch64.img`,
`target/riscv64.img`; `write_boot_disk` in `src/limine_disk.rs`) are what an
installed myos looks like: a GPT disk laid out for upgrades that swap the
kernel and the initramfs (issue #351), with room for state that survives
them.

| Partition | Type | Size | myos sees it as (a virtio disk) | Holds |
|-----------|------|------|---------------------------------|-------|
| 1 `BIOS Boot` | BIOS boot | 1 MiB at LBA 2048 | `/dev/vda/p1` | Limine's BIOS stage 2 (`limine bios-install`, x86) |
| 2 `EFI System` | ESP (`c12a7328-…`) | 512 MiB, FAT32 | `/dev/vda/p2`, mounted at `/boot` | Limine, the boot slots, `limine.conf` and `fstab` |
| 3 `myos data` | Linux data (`0fc63daf-…`) | 64 MiB (the rest of the disk after `--install`), ext2 | `/dev/vda/p3`, mounted at `/data` | state an upgrade keeps: `get-myos`'s apps, `get-alpine`'s root |

The image is 579 MiB, written sparse: only what the partitions hold takes
space on the host.

## The ESP: two boot slots

```
EFI/BOOT/BOOTX64.EFI          Limine (BOOTAA64.EFI, BOOTRISCV64.EFI)
boot/limine/limine.conf       the only Limine config
boot/limine/limine-bios.sys   x86: Limine's BIOS stage 3
boot/a/kernel                 slot a: the kernel (its loadable segments)
boot/a/initramfs              slot a: the initramfs
boot/a/version                slot a: its release, the initramfs's /lib/myos-release
boot/b/                       slot b: empty until an upgrade writes it
boot/virt-aarch64.dtb         aarch64, riscv64 (virt.dtb): the device tree
fstab                         what `mount -a` mounts at boot (below)
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
    cmdline: slot=b

/myos a
    protocol: limine
    path: boot():/boot/a/kernel
    module_path: boot():/boot/a/initramfs
    cmdline: slot=a
```

With two entries the menu waits three seconds, so the other slot can be
picked at the console when the active one does not boot; with one (a fresh
image: only slot `a`) it boots at once. aarch64 and riscv64 add their
`global_dtb` (and riscv64 `paging_mode`) lines, as `src/main.rs` writes them.
An entry's `cmdline: slot=X` is how the running system knows which slot it
came from (`/proc/cmdline`), the fallback picked by hand included.

An upgrade writes the slot that is not running, then rewrites
`boot/limine/limine.conf` with that slot first. That file is the switch, and
the only file the switch touches: Limine reads it from no other place (the
images used to carry copies at `/limine.conf` and `/EFI/BOOT/limine.conf`).
Writing a slot's files leaves the running one, and the config pointing at
it, as they were until the switch.

## Upgrading: `get-myos --upgrade`

```sh
get-myos --upgrade        # [-m MIRROR] [-f]
```

On the running system, as root:

1. The running slot is the `slot=` of `/proc/cmdline`; the boot disk's ESP is
   the ESP partition (`/proc/partitions`) whose `boot/<slot>/version` is
   this system's `/lib/myos-release`: the one `mount -a` mounted at
   `/boot`, used there (it is mounted for the while when it is not).
2. The mirror's `<arch>-boot.txt` (`docs/packages.md`) names the release's
   kernel and initramfs with their sizes and SHA-256. When its release is
   not newer than the running slot's version, there is nothing to do (`-f`
   writes the other slot anyway).
3. The other slot's `version` goes first. Each file is downloaded into
   memory and checked against the list before it is written into the slot
   (nothing unchecked reaches the ESP, and a slow disk does not stall the
   download); the ESP is unmounted and mounted again, and what the disk has
   is checked against the list too. Then the slot's `version`.
4. Limine's files on the ESP are brought to the release's (the `esp` lines
   of the list, `limine.conf` aside), those whose SHA-256 differs: each new
   one is written beside the old (`BOOTX64.EFI.new`), all of them are read
   back after a remount, then each is renamed over the old one (a FAT rename
   rewrites one directory entry). On x86 a new `limine-bios.sys` reruns
   `limine bios-install` on the boot disk: the BIOS stage (the MBR code and
   stage 2) must be the same version as that file. It says what it
   replaced.
5. `limine.conf` is rewritten (beside it, then renamed over it) with the new
   slot first and the running one as the fallback, `timeout: 3`: the global
   lines and the entries come from the running config, or, when step 4
   replaced anything, from the release's (a new Limine may read its config
   differently; lines edited by hand are lost then).
6. An ESP get-myos mounted itself is unmounted. It does not reboot:
   `reboot` starts the new slot.

A failure before step 4 leaves the running slot, Limine and the config as
they were; the half-written slot has no `version`. Step 4 is the one that is
not atomic: the renames happen one after the other, and `limine bios-install`
rewrites sectors in place, so a power cut in that moment can leave a disk
whose Limine pieces do not match. It runs only when the release's Limine
differs, which is rare (a new Limine pin). The release's initramfs has no
Linux layer (the default build's): an upgraded system gets it back with an
image built with `--features linux_compat`.

## Installing on another disk: `get-myos --install DISK`

```sh
get-myos --install nvme1n1    # or /dev/nvme1n1, /dev/nvme1n1/data
```

Erases DISK and lays the boot disk out on it: the GPT (BIOS boot 1 MiB,
the ESP 512 MiB, the data partition over the rest of the disk, ending on a
MiB), read by the kernel at once (a `/proc/pci` rescan); `mkfs.fat` on the
ESP and `mkfs.ext2` on the data partition; Limine's files from the release
(the `esp` lines of `<arch>-boot.txt`: the EFI binary, x86's
`limine-bios.sys`, the device tree, `startup.nsh`, and a `limine.conf` for
slot `a` alone), each checked like the slot's; then step 3 of the upgrade
fills slot `a` from the mirror. On x86 it then runs `limine bios-install`
(Limine's own tool, the `limine` port, at `/bin/etc/limine`) for the BIOS
stage: Limine's code in the MBR and its stage 2 in the BIOS boot partition,
what the host does for the images; the disk boots by BIOS and by UEFI. It
refuses the running boot disk, a disk with a partition mounted, and one
under 580 MiB.

Nothing comes from the running system's ESP, so a system booted from the
ISO (`cargo run -- iso`, a live medium) installs the same: boot it, then
`get-myos --install` the disk.

### Without a network: `--install DISK --local`

```sh
get-myos --install nvme1n1 --local
```

Installs the running system itself, with no mirror: the kernel and the
initramfs this boot came from, which the kernel shows at `/proc/boot/kernel`
and `/proc/boot/initramfs` (what Limine loaded, kept in memory for the
kernel's life, so it works however the system booted: the ISO from a CD or
a USB stick, a disk), and Limine's files from the initramfs, which carries
the ESP's at `/lib/myos-boot/` with a `boot.txt` in the release list's
format. get-myos writes the list of those (`.get-myos/local-boot` in its
root, `docs/packages.md`, the files named by their paths) and installs from it as from
a mirror's: the same layout, checks and BIOS stage; slot `a` gets this
system's release (`/lib/myos-release`). From the ISO, that is the ISO's
system, the Linux layer included (the ISO is built with it); a later
`--upgrade` from a mirror brings the release's initramfs, which has none.

## Mounted at boot: `/boot`, `/data` and the fstab

init runs `mount -a` before it starts anything else (`user/mount`):

1. The ESP the system booted from is mounted at `/boot`: Limine tells the
   kernel the GPT partition it loaded the kernel from, which it shows at
   `/proc/boot/partuuid` (that partition's unique GUID, as
   `/proc/partitions` lists it). Booted from the ISO there is none, and
   nothing below happens.
2. `/tmp/mnt` is bound over `/mnt`: the root is the read-only initramfs,
   so this is where mount points can be made. A disk mounted at
   `/mnt/disk` is listed at `/tmp/mnt/disk` in `/proc/mounts`.
3. Each line of `/boot/fstab` (`fstab` at the ESP's root) is mounted,
   unless its partition is mounted already:

```
# What `mount -a` mounts at boot (docs/install.md): PARTUUID=<guid> MOUNTPOINT FSTYPE [rw],
# the partition's unique GUID from /proc/partitions; a mount point under /mnt is made.
PARTUUID=6d796f73-0000-4000-8000-000000000004 /data ext2
```

A partition is named only by its unique GUID (`PARTUUID=`): a disk's name
(`nvme0n1`, `vda`) depends on the machine and on what else is plugged in,
the GUID does not. A line naming a partition that is not there (a USB disk
left out) is skipped; a line that is wrong (a `/dev/` path, an unknown
option: the kernel has no read-only mounts, so `rw` is the only one) or that
does not mount is reported. Neither stops the boot, and init says
`mount -a: not everything mounted`. The mount point must exist, or be
under `/mnt`, where it is made.

The image builder writes the fstab of the images and `get-myos --install`
that of the disk it lays out, each naming that disk's data partition. The
images' partitions have fixed GUIDs (`6d796f73-…-000000000004` is the data
partition): two disks written from the same image both match their fstab
lines, and the first one found is mounted. Nothing else writes the fstab:
`--upgrade` leaves it as it is.

To have a disk mounted at every boot, add its line on the running system
and try it at once (the ESP is mounted read-write):

```sh
grep nvme1n1/ /proc/partitions      # the fifth field: the partition's unique GUID
echo 'PARTUUID=<that guid> /mnt/disk ext2' >> /boot/fstab
mount -a                            # mounts what is not mounted yet
```

`mount PARTUUID=<guid> DIR FSTYPE` mounts one by hand. A filesystem on a
whole disk or in an MBR partition has no partition GUID and cannot be
listed: naming one by its filesystem's own ID (Linux's `UUID=`) would be
the way, if it is ever needed.

## The data partition

An empty ext2 (`ext2fs::mkfs`, the same code as myos's `mkfs.ext2`) at
`/data`, for what must outlive an upgrade. It holds the default roots of
the package tools: `get-myos`'s apps in `/data/apps` (`docs/packages.md`)
and `get-alpine`'s Alpine root in `/data/alpine` (`docs/linux-compat.md`),
both labelled `sys.pkg`. Other candidates are SSH host keys and
`authorized_keys` (in `/tmp` for now, `docs/ssh.md`), `/etc` overrides and
home directories; which of those move there is its own change. Only the
administrator may write it (the policy's `sys.file` label). On a disk larger
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
`boot/` and none of this layout, and installs from it with `get-myos
--install` (above; `--local` when it has no network).

## Inside the boot tests

The boot tests boot a copy of the image (`target/boot-test-<name>.img`, so
the build's own stays as built), slot `a`. myos sees the boot disk on every
arch (a virtio disk: on x86 the third, `vdc`, after the test disks), and
`kernel.sh`'s `boot_disk` checks its layout, as `mount -a` mounted it: the
ESP at `/boot` (`/proc/boot/partuuid`), the slots, the one `limine.conf`,
slot `a`'s version and `/proc/cmdline`, the fstab naming the data
partition, the empty ext2 at `/data`; `fstab_mount` adds lines to
`/boot/fstab` (a scratch partition under `/mnt`, a `/dev/` path, a missing
GUID) and checks what `mount -a` does with each. It also checks that `/proc/boot/` holds slot `a`'s kernel and
initramfs, and `/lib/myos-boot/` the ESP's Limine files. Every list ends
with `get-myos --install --local` on the scratch disk. The full list runs
`get-myos --upgrade -f` against the host's mirror (this build's boot files)
after making the disk's Limine stale (its EFI binary changed; on x86
`limine-bios.sys` too and the BIOS stage erased), so the upgrade has to
replace it; then `get-myos --install` on the scratch disk from the mirror,
and last the local install over it (`user/get-myos/test.sh`). When it
passed, the launcher boots the disk again (`run.sh reboot`), which must come
up from slot `b` at its release, with the Limine the upgrade wrote, and then
the disk `--install --local` made (`run.sh installed`), from its slot `a`
(either with its own data partition at `/data`, from its fstab):
by BIOS on the bios job (the BIOS stages `limine bios-install` wrote), by
UEFI on the others. The ISO is not booted in CI: an install from it runs
the same code.
