# Installing: the boot disk

The disk images (`target/bios.img`, `target/uefi.img`,
`target/aarch64.img`, `target/riscv64.img`; `write_boot_disk` in
`src/limine_disk.rs`) are what an installed myos looks like. Each is a
GPT disk laid out for upgrades that swap the kernel and the initramfs
(issue #351), with room for state that survives them. This page covers
the layout, upgrading, installing on another disk, what is mounted and
run at boot, and how the boot tests check all of it.

| Partition | Type | Size | myos sees it as (a virtio disk) | Holds |
|-----------|------|------|---------------------------------|-------|
| 1 `BIOS Boot` | BIOS boot | 1 MiB at LBA 2048 | `/dev/vda/p1` | Limine's BIOS stage 2 (`limine bios-install`, x86) |
| 2 `EFI System` | ESP (`c12a7328-…`) | 512 MiB, FAT32 | `/dev/vda/p2`, mounted at `/boot` | Limine, the boot slots, `limine.conf` and `fstab` |
| 3 `myos data` | Linux data (`0fc63daf-…`) | 64 MiB (the rest of the disk after `--install`), ext2 | `/dev/vda/p3`, mounted at `/data` | state an upgrade keeps: `get-myos`'s apps, `get-alpine`'s root |

The image is 579 MiB, written sparse: only what the partitions hold
takes space on the host. A VPS whose disk hangs off a virtio-scsi
controller (Hetzner's do) sees it as `/dev/sda` instead
(`modules/virtio_scsi`). The fstab names the partitions by GUID, so the
layout does not depend on the disk's name.

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
boot/tests                    the build's disks only: the tests module
fstab                         what `mount -a` mounts at boot (below)
```

Each slot holds a kernel and the initramfs built with it. The initramfs
is a zstd frame the kernel decodes at boot (`kernel/src/fs/zstd.rs`,
issue #621). The two files take some 6 MiB together, about 17 MiB once
the initramfs is decoded, so the ESP has room for both slots and to
grow.

The build's disks (`target/*.img`) also carry the tests module
(`docs/testing.md`), at `boot/tests` beside the slots. Each entry of
their config loads it after the slot's initramfs
(`module_path: boot():/boot/tests`). `get-myos` writes that line into
every entry it makes exactly when the ESP has the file, so an upgrade of
a test disk keeps it. A disk installed from a mirror never has one.

### `limine.conf`

`limine.conf` has an entry per filled slot, the active one first
(Limine's `default_entry: 1`):

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

- With two entries, the menu waits three seconds, so the other slot can
  be picked at the console when the active one does not boot.
- With one entry (a fresh image: only slot `a`), it boots at once.
- aarch64 and riscv64 add their `global_dtb` line, and riscv64 its
  `paging_mode` line, as `src/main.rs` writes them.
- An entry's `cmdline: slot=X` is how the running system knows which
  slot it came from (`/proc/cmdline`), including a fallback picked by
  hand.

An upgrade writes the slot that is not running, then rewrites
`boot/limine/limine.conf` with that slot first. That file is the switch,
and the only file the switch touches; Limine reads its config from no
other place. Writing a slot's files leaves the running slot, and the
config pointing at it, as they were until the switch.

## Upgrading: `get-myos --upgrade`

```sh
get-myos --upgrade        # [-m MIRROR] [-f]
```

Run it on the running system, as root. It goes through these steps:

1. **Find the boot disk.** The running slot is the `slot=` of
   `/proc/cmdline`. The boot disk's ESP is the ESP partition
   (`/proc/partitions`) whose `boot/<slot>/version` is this system's
   `/lib/myos-release`. That is normally the one `mount -a` mounted at
   `/boot`, and get-myos uses it there. When it is not mounted, get-myos
   mounts it for the duration.
2. **Read the release.** The mirror's `<arch>-boot.txt`
   (`docs/packages.md`) names the release's kernel and initramfs with
   their sizes and SHA-256. When its release is not newer than the
   running slot's version, there is nothing to do. `-f` writes the other
   slot anyway.
3. **Write the other slot.** Its `version` is removed first. Each file is
   downloaded into memory and checked against the list before it is
   written into the slot. So nothing unchecked reaches the ESP, and a
   slow disk does not stall the download. Then the ESP is unmounted and
   mounted again, and what the disk has is checked against the list too.
   The slot's `version` is written last.
4. **Update Limine.** Limine's files on the ESP are brought to the
   release's (the `esp` lines of the list, `limine.conf` aside). Only
   those whose SHA-256 differs are replaced:
   - each new file is written beside the old one (`BOOTX64.EFI.new`);
   - all of them are read back after a remount;
   - then each is renamed over the old one (a FAT rename rewrites one
     directory entry).

   On x86 a new `limine-bios.sys` reruns `limine bios-install` on the
   boot disk, because the BIOS stage (the MBR code and stage 2) must be
   the same version as that file. get-myos says what it replaced.
5. **Switch.** `limine.conf` is rewritten (beside it, then renamed over
   it) with the new slot first, the running one as the fallback, and
   `timeout: 3`. The global lines and the entries come from the running
   config. When step 4 replaced anything they come from the release's
   config instead, since a new Limine may read its config differently;
   lines edited by hand are lost then.
6. **Clean up.** An ESP that get-myos mounted itself is unmounted.
   get-myos does not reboot: `reboot` starts the new slot.

A failure before step 4 leaves the running slot, Limine and the config
as they were; the half-written slot has no `version`.

Step 4 is the one step that is not atomic. The renames happen one after
the other, and `limine bios-install` rewrites sectors in place, so a
power cut at that moment can leave a disk whose Limine pieces do not
match. It runs only when the release's Limine differs, which is rare (a
new Limine pin).

The release's initramfs is the default build's, without the Linux layer.
An upgraded system gets the layer back with an image built with
`--features linux_compat`.

## Installing on another disk: `get-myos --install DISK`

```sh
get-myos --install nvme1n1    # or /dev/nvme1n1, /dev/nvme1n1/data
```

This erases DISK and lays the boot disk out on it:

1. the GPT: BIOS boot 1 MiB, the ESP 512 MiB, and the data partition
   over the rest of the disk, ending on a MiB. The kernel reads it at
   once (a `/proc/pci` rescan);
2. `mkfs.fat` on the ESP and `mkfs.ext2` on the data partition;
3. Limine's files from the release, each checked like the slot's. These
   are the `esp` lines of `<arch>-boot.txt`: the EFI binary, x86's
   `limine-bios.sys`, the device tree, `startup.nsh`, and a
   `limine.conf` for slot `a` alone;
4. slot `a`, filled from the mirror as in step 3 of the upgrade;
5. on x86, `limine bios-install` for the BIOS stage: Limine's code in the
   MBR and its stage 2 in the BIOS boot partition, as the host does for
   the images. This is Limine's own tool, the `limine` port, at
   `/bin/etc/limine`. The disk then boots by BIOS and by UEFI.

It refuses the running boot disk, a disk with a partition mounted, and a
disk under 580 MiB.

Nothing comes from the running system's ESP. So a system booted from the
ISO (`cargo run -- iso`, a live medium) installs the same way: boot it,
then `get-myos --install` the disk.

### Without a network: `--install DISK --local`

```sh
get-myos --install nvme1n1 --local
```

This installs the running system itself, with no mirror:

- The kernel and the initramfs come from this boot. The kernel shows
  them at `/proc/boot/kernel` and `/proc/boot/initramfs`: what Limine
  loaded, kept in memory for the kernel's life. So it works however the
  system booted: the ISO from a CD or a USB stick, or a disk.
- Limine's files come from the initramfs, which carries the ESP's at
  `/lib/myos-boot/`, with a `boot.txt` in the release list's format.

get-myos writes the list of those files (`.get-myos/local-boot` in its
root, `docs/packages.md`; the files are named by their paths). It then
installs from that list as from a mirror's: the same layout, checks and
BIOS stage. Slot `a` gets this system's release (`/lib/myos-release`).

- From the ISO, that is the ISO's system, the Linux layer included (the
  ISO is built with it). A later `--upgrade` from a mirror brings the
  release's initramfs, which has no Linux layer.
- A system booted with the tests module (`/proc/boot/tests`, a test
  disk) installs it too. The list names it as an `esp` file
  (`boot/tests`). In place of the initramfs's `limine.conf`, it uses a
  copy that loads the module (`.get-myos/local-boot.limine.conf`).

## Mounted at boot: `/boot`, `/data` and the fstab

init runs `mount -a` (`user/mount`) before it starts anything else:

1. The ESP the system booted from is mounted at `/boot`. Limine tells the
   kernel which GPT partition it loaded the kernel from, and the kernel
   shows it at `/proc/boot/partuuid` (that partition's unique GUID, as
   `/proc/partitions` lists it). Booted from the ISO there is none, and
   nothing below happens.
2. `/tmp/mnt` is bound over `/mnt`. The root is the read-only initramfs,
   so this is where mount points can be made. A disk mounted at
   `/mnt/disk` is listed at `/tmp/mnt/disk` in `/proc/mounts`.
3. Each line of `/boot/fstab` (`fstab` at the ESP's root) is mounted,
   unless its partition is mounted already:

```
# What `mount -a` mounts at boot (docs/install.md): PARTUUID=<guid> MOUNTPOINT FSTYPE [rw],
# the partition's unique GUID from /proc/partitions; a mount point under /mnt is made.
PARTUUID=6d796f73-0000-4000-8000-000000000004 /data ext2
```

A partition is named only by its unique GUID (`PARTUUID=`). A disk's name
(`nvme0n1`, `vda`) depends on the machine and on what else is plugged in;
the GUID does not.

- A line naming a partition that is not there (a USB disk left out) is
  skipped.
- A line that is wrong is reported: a `/dev/` path, or an unknown
  option. The kernel has no read-only mounts, so `rw` is the only
  option.
- A line that does not mount is reported too.

Neither stops the boot; init says `mount -a: not everything mounted`.
The mount point must exist, or be under `/mnt`, where it is made.

The image builder writes the fstab of the images, and
`get-myos --install` writes that of the disk it lays out. Each names that
disk's data partition. Nothing else writes the fstab: `--upgrade` leaves
it as it is. The images' partitions have fixed GUIDs
(`6d796f73-…-000000000004` is the data partition). So two disks written
from the same image both match their fstab lines, and the first one found
is mounted.

To have a disk mounted at every boot, add its line on the running system
and try it at once (the ESP is mounted read-write):

```sh
grep nvme1n1/ /proc/partitions      # the fifth field: the partition's unique GUID
echo 'PARTUUID=<that guid> /mnt/disk ext2' >> /boot/fstab
mount -a                            # mounts what is not mounted yet
```

`mount PARTUUID=<guid> DIR FSTYPE` mounts one by hand. A filesystem on a
whole disk or in an MBR partition has no partition GUID and cannot be
listed. Naming it by its filesystem's own ID (Linux's `UUID=`) would be
the way, if it is ever needed.

## The boot script: `/etc/rc` and `/data/etc/rc.local`

After `mount -a` and netd, init starts `/etc/rc` (`etc/rc` in the
repository). It goes on to the logins without waiting for it
(`[ OK ] rc started (in the background)`). The script's lines go to the
console's screen (`Alt+0`, `docs/tty.md`), not to the ttys' logins. Its
last line says the boot is over, and is also written to
`/tmp/boot.done`:

```
[ OK ] boot complete (/data/etc/rc.local)
[ WARN ] boot complete (/data/etc/rc.local exited 1)
```

The script runs as root, in `admin`. init starts it as `system`, and the
policy's `exec /etc/rc -> rc` rule puts it in the `rc` domain. That
domain's only power is entering a user without a password, and the
script uses it at once (`sec as root`, `docs/security.md`). Then it:

1. binds a writable directory over `/etc/dropbear`, for dropbear's host
   keys and `authorized_keys` (`docs/ssh.md`). That is `/data/dropbear`
   when the data partition is mounted, so they survive reboots and
   upgrades, else `/tmp/dropbear`, which lasts until the next boot. Both
   are labelled `secret(root)`: only root reads them;
2. binds the global apps of `/data/apps` again
   (`get-myos -r /data/apps --global`, `docs/packages.md`), so their
   programs are on `PATH` before the first login. Its output goes to
   `/tmp/get-myos-global.log`;
3. when the data partition has an app list, `/data/apps/apps.list`,
   starts `get-myos -r /data/apps --sync` in the background, so the apps
   are what the list says (`docs/packages.md`), from the mirror they were
   installed from. Its output goes to `/tmp/get-myos-sync.log`;
4. copies the image's `/etc/rc.local` to `/data/etc/rc.local` when that
   is missing, and runs it with `sh`. Without a data partition, the
   image's own runs.

`/data/etc/rc.local` belongs to the administrator, who edits it on the
running system (only `admin` may write `/data`). When it is deleted, the
default comes back at the next boot. The default loads the Linux layer
(`docs/linux-compat.md`) unless the kernel did already, and starts
dropbear on port 2222:

```sh
grep -q '^linux$' /proc/modules || insmod /lib/modules/linux
dropbear -R -D /etc/dropbear -p 2222
```

A command that stays in the foreground there holds back the boot's last
line, not the login. Start daemons in the background: `&`, or a
program's own `-d`. dropbear goes to the background unless given `-F`.

## The data partition

The data partition is an empty ext2 (`ext2fs::mkfs`, the same code as
myos's `mkfs.ext2`) at `/data`, for what must outlive an upgrade. Only
the administrator may write it (the policy's `sys.file` label). It holds:

- the default roots of the package tools, both labelled `sys.pkg`:
  `get-myos`'s apps in `/data/apps` (`docs/packages.md`) and
  `get-alpine`'s Alpine root in `/data/alpine` (`docs/linux-compat.md`);
- the boot script's `/data/etc/rc.local` and dropbear's keys in
  `/data/dropbear` (above).

Other candidates are `/etc` overrides and home directories; moving any of
those there is a change of its own.

On a disk larger than the image (a VPS), the space after the image is
left unused. Growing the partition to the disk needs the GPT rewritten
and the ext2 grown (issue #488). So a disk written from the image has
64 MiB of `/data`. That is room for get-myos's apps, but not for an
Alpine root: a browser's takes some hundreds of MiB, and apk stops with
`No space left on device`. A VPS that needs one installs with
`get-myos --install` from the ISO (below), which gives the data partition
the rest of the disk.

## The first install

From a rescue system (a VPS provider's, a live Linux), write the image
over the disk. `cargo build` makes it; `uefi.img` and `bios.img` are the
same disk, both BIOS and UEFI bootable.

```sh
zstd -c target/uefi.img | ssh root@rescue 'zstd -d | dd of=/dev/sda bs=1M conv=fsync'
```

`/dev/sda` is the rescue system's Linux name for the disk. zstd keeps the
transfer to what the image holds. `dd` writes all of it, zeros included,
so nothing of what the disk held before shows through.

The GPT's backup is at the end of the image, not of the disk. Tools that
check it (`sgdisk -e`, `parted`) offer to move it to the end, which is
safe.

A VPS without a rescue system but with custom ISOs boots the hybrid ISO
(`cargo run -- iso`). That is a live medium with the kernel and the
initramfs at `boot/` and none of this layout. Install from it with
`get-myos --install` (above; with `--local` when it has no network).

## Inside the boot tests

The boot tests boot a copy of the image (`target/boot-test-<name>.img`,
so the build's own stays as built), from slot `a`. myos sees the boot
disk on every arch as a virtio disk; on x86 it is the third, `vdc`, after
the test disks.

`kernel.sh` checks the disk:

- `boot_disk` checks its layout, as `mount -a` mounted it: the ESP at
  `/boot` (`/proc/boot/partuuid`), the slots, the one `limine.conf`,
  slot `a`'s version and `/proc/cmdline`, the fstab naming the data
  partition, and the ext2 at `/data` with only what the boot script put
  there. It also checks that `/proc/boot/` holds slot `a`'s kernel and
  initramfs, and `/lib/myos-boot/` the ESP's Limine files.
- `boot_rc` checks the boot script's work: the bind, `rc.local`, the
  Linux layer and dropbear.
- `fstab_mount` adds lines to `/boot/fstab` (a scratch partition under
  `/mnt`, a `/dev/` path, a missing GUID) and checks what `mount -a`
  does with each.

`user/get-myos/test.sh` runs the installs:

- Every list ends with `get-myos --install --local` on the scratch disk.
- The full list first makes the disk's Limine stale: its EFI binary
  changed, and on x86 `limine-bios.sys` too and the BIOS stage erased.
  It then runs `get-myos --upgrade -f` against the host's mirror (this
  build's boot files), so the upgrade has to replace Limine. Then it
  runs `get-myos --install` on the scratch disk from the mirror, and last
  the local install over it.

When that passed, the launcher boots twice more:

1. the disk again (`run.sh reboot`), which must come up from slot `b` at
   its release, with the Limine the upgrade wrote;
2. the disk that `--install --local` made (`run.sh installed`), from its
   slot `a`.

Each comes up with its own data partition at `/data`, from its fstab.
They boot by BIOS on the bios job (using the BIOS stages that
`limine bios-install` wrote) and by UEFI on the others. The ISO is not
booted in CI: an install from it runs the same code.

## History

- The images used to carry copies of `limine.conf` at `/limine.conf` and
  `/EFI/BOOT/limine.conf`. Only `boot/limine/limine.conf` remains, so the
  upgrade's switch is one file.
