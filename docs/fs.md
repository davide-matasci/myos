# Filesystems and disks

myos has one file tree. The read-only image sits at `/`, and the kernel's
own filesystems (`/tmp`, `/dev`, `/proc`) and the disks' filesystems are
mounted over it. This page covers the VFS and its mounts, open files and
locks, the kernel's filesystems, disks and partitions, the page and block
caches, and the FAT and ext2 modules.

## The VFS and mounts

The VFS keeps a mount table. A path goes to the mount with the longest
matching prefix. `vfs::mounts_text()` exports the table as `/proc/mounts`.

A bind mount shows a directory or a file at a second place too:
`mount SRC TARGET bind`. The target need not exist.

Every path the syscalls touch is checked against the security policy
(`kernel/src/sec`, `/etc/policy` from `etc/policy`) and against the
caller's namespace (`kernel/src/task/ns.rs`, which also implements
`chroot`). Labels come from the canonical path, with bind mounts resolved,
so a second name gives no second label (`docs/security.md`).

### Mounting a disk

```sh
mount /dev/sda/data /mnt ext2
mount PARTUUID=<guid> /mnt ext2      # the partition's unique GUID
umount /mnt
```

- The target is any existing directory that is not a mount point yet, at
  the top level or anywhere below (`mkdir /tmp/usb`).
- `/mnt` is `/tmp/mnt` bound over the image's, so `mkdir /mnt/usb` works
  too.
- A disk is mounted once.
- At boot, init's `mount -a` mounts the boot disk's ESP at `/boot`, what
  its `/boot/fstab` lists, and the data partition at `/data`
  (`docs/install.md`).
- `umount DIR` gives the directory back once no file on the disk is open
  or mapped and nothing is mounted below it. The filesystem's `unmount`
  hook first writes back what it caches (ext2).
- A directory a mount hangs from can be neither renamed nor removed.
- The kernel's own trees (`/`, `/tmp`, `/dev`, `/proc`) cannot be
  unmounted.

## Open files

An fd, a file mapping or a running program holds its file, not its name
(`kernel/src/fs/node.rs`):

- It follows a rename of the file or of a directory above it.
- A file unlinked or replaced (`rename` over it) while held stays readable
  and writable through the holder until the last holder lets go. tmpfs
  keeps such a file under a name no path reaches. A module filesystem
  keeps its inode with the `unlink_keep` hook, as ext2 does. On the other
  filesystems the holder's reads and writes fail instead.

Some filesystems hand out file ids: tmpfs for its files, and a module
through its `file_id` hook (ext2's inode numbers). Once such a file is
open, the kernel reads, writes, sizes and times it through the id. It looks
the file up by path only when it is opened. The node table shares one node
per file, and `stat`'s `st_ino` is the id.

Renames and unlinks exclude the calls that use a path or an open file, so
none of those calls reaches a new file that took an old name.

There are no hard links, on purpose: a file has one name
(`docs/security.md`), and `link` fails with `EPERM`.

## File locks

File locks (`kernel/src/fs/lock.rs`) are advisory, as on Unix. There are
two kinds, and they never meet:

- `flock` locks belong to the open file description. A `dup` or a `fork`
  shares them, and they go with the description's last fd.
- `fcntl` record locks (`F_SETLK`, `F_SETLKW`, `F_GETLK`, libc's `lockf`)
  lock byte ranges for the process. As POSIX says, they go when the process
  exits or closes any fd on the file. Linux's open-file-description record
  locks belong to the description instead.

A request that conflicts either fails with `EWOULDBLOCK` without waiting,
or waits until the holder lets go. A caught signal ends the wait (`EINTR`).

## The kernel's filesystems

### rootfs

rootfs is the read-only tree at `/`: every file of the image in one sorted
table (`kernel/src/fs/rootfs.rs`). Its files come from:

- the initramfs, the cpio archive Limine loads next to the kernel. It
  carries the kernel modules too (`/lib/modules`, `docs/modules.md`). It is
  a zstd frame that the kernel decodes once at boot into pages of its own,
  and then unpacks by reference, without a copy (`kernel/src/fs/zstd.rs`);
- the tests module, on the build's disks (`docs/testing.md`), which joins
  the same tree;
- a module's `vfs_register`, which adds or replaces a file at run time.

The kernel embeds no program: it starts `/bin/custom/init` from the
initramfs. A directory is implied by its files, so an empty one carries a
`.keep` file (`mnt/.keep`). `/tmp`, `/dev`, `/proc` and the module
filesystems are mounted over rootfs.

### tmpfs and devfs

tmpfs is the writable mount at `/tmp`, where `O_CREAT` creates files. A
tmpfs file is bytes in the kernel heap, up to 128 MiB per file (issue
#338). devfs at `/dev` holds the device nodes.

### procfs

procfs files are generated when they are read, not stored bytes:

| File | What |
|------|------|
| `/proc/mounts` | the mount table |
| `/proc/partitions` | the disks' partitions ("Disks and partitions" below) |
| `/proc/cmdline` | Limine's `cmdline:`, which names the boot slot |
| `/proc/boot/kernel`, `/proc/boot/initramfs` | the files this boot came from |
| `/proc/boot/tests` | the tests module, on the build's disks (`docs/testing.md`) |
| `/proc/boot/partuuid` | the partition this boot came from (mounted at `/boot`) |
| `/proc/<pid>/status`, `/proc/<pid>/task/<tid>/status` | every process and its threads, with names and CPU times (`docs/proc.md`) |
| `/proc/cpu` | the CPUs' idle time (`docs/proc.md`) |
| `/proc/self/ctx` | the caller's uid, user and domain |
| `/proc/sys/security/users` | the policy's users, what libgloss's `getpwnam` and friends read |
| `/proc/sys/kernel/hostname` | the host name, writable, up to 64 bytes |
| `/proc/sys/kernel/overflowuid`, `overflowgid` | 65534 (what bubblewrap reads) |

The host name is "myos" at boot. `echo name > /proc/sys/kernel/hostname`
or `hostname name` sets it.

## Disks and partitions

The disk drivers are modules that register their disks through
`blk_register`. They load before the filesystem modules.

| Driver | Disks |
|--------|-------|
| virtio-blk | `vda`, `vdb`, ... |
| virtio-scsi, USB storage | `sda`, `sdb`, ... |
| NVMe | `nvme0n1`, ... |
| SD | `mmcblk0` |

The kernel hands out the letter of `sd?` disks, to SCSI disks and USB
sticks alike. The virtio drivers are the `virtio-drivers` crate, bound to
the module ABI by `modules/virtio`.

A disk is a directory, `/dev/<disk>/` (`kernel/src/blk.rs`,
`kernel/src/blk/gpt.rs`):

- `data` is the whole disk.
- `p<N>` is the partition in entry `N` of its GUID partition table. The
  numbers follow the entries, so a gap stays a gap. A partition is a block
  device of its own, confined to its sectors.
- `info` is a line about the disk. `cat /dev/*/info` shows all of them.

An `info` line gives the disk's name, its size in bytes and what its driver
says of it. That is where the disk is (a PCI function, a virtio-mmio
window, a SCSI target and LUN behind its host, the `/proc/usb` device and
its vendor:product) and what the device reports itself to be. A read-only
virtio disk says `ro`. Examples:

```
nvme0n1 1610612736 nvme pci 00:04.0 model "QEMU NVMe Ctrl" serial "myos"
vda 8589934592 virtio_blk pci 00:03.0 serial "…"
sda 1048576 virtio_scsi pci 00:05.0 target 0:0 vendor "QEMU" model "QEMU HARDDISK"
sdb 67108864 usb_storage usb 0x002 46f4:0001 vendor "QEMU" model "QEMU HARDDISK"
```

The kernel reads the partition table on the first look at `/dev` after the
disk appeared. It checks both CRC32s and uses the backup at the end of the
disk when the primary is bad. MBR tables are not read.

`/proc/partitions` lists the partitions, one line each: first sector,
bytes, type GUID, unique GUID and name.

```
nvme1n1/p1 2048 20971520 c12a7328-f81f-11d2-ba4b-00a0c93ec93b <unique GUID> "EFI system"
```

A disk's partitions go with it (a USB stick pulled out). The kernel refuses
that while one of them is mounted or open. Writing `rescan` to `/proc/pci`
reads the tables again for the disks none of whose partitions is in use.

## Caches and shared memory

### Block cache

What the kernel reads from a disk stays in 4 KiB pages, up to an eighth of
RAM. The least recently used page goes out first, and the whole cache is
given back when memory runs out. Writes go to the disk and update the
cache. `BlockCacheKiB` in `/proc/meminfo` shows its size.

### Page cache

A file's pages that processes map without write permission (programs' and
libraries' code and constant data) are one frame each
(`kernel/src/fs/pagecache.rs`). Every mapping of them shares the frame, and
the cache keeps it for the next process. A write, a truncation or an unlink
drops the pages. The cache holds up to a quarter of RAM (`PageCacheKiB` in
`/proc/meminfo`).

A `MAP_SHARED` mapping of a regular file through a writable fd is the file
itself:

- Its pages are the cache's frames. Every process that maps them writes
  through them.
- `read` sees the changes and `write` updates them at once.
- `msync`, `munmap`, exit and exec write them back to the file.
- A file unlinked while an fd or a mapping holds it keeps its pages for
  them.

A `MAP_SHARED | MAP_ANON` mapping is a tmpfs file no name reaches. It is
shared with forked children and goes with its last mapping.

### Shared memory

POSIX shared memory is `/dev/shm`, a tmpfs directory bound there:

- `shm_open`, `shm_unlink` and `memfd_create` in libgloss. `memfd_create`
  creates the file and unlinks it at once.
- System V `shmget`, `shmat`, `shmdt` and `shmctl`. A segment is a file
  there, and its id is the file's inode number. Its mode reads as 0666,
  since the policy decides the file's protection.

A mapping's frames are written back to the tmpfs file's bytes in the kernel
heap, so a segment costs its size twice, up to the tmpfs file cap of
128 MiB (issue #338).

### Fork is copy-on-write

A forked child maps its parent's pages (image, stack, heap, private
mappings) rather than copies of them:

- Both sides lose write permission to a shared page (`LEAF_COW` in the leaf
  entry). `kernel/src/user/cow.rs` counts the page's mappings.
- The first store by either side, from userspace or by a kernel copy into
  the page, gives that side its own copy (`user::cow_break`). When the
  other side's mapping has gone, the side keeps the frame itself.
- A shared mapping's pages and a device's pages are the same in both.
- Exec in place replaces the shared frames it keeps before it writes the
  new image.

`Forks` and `CowCopies` in `/proc/meminfo` count the forks and the pages
copied.

## FAT

The FAT module reads and writes FAT16 and FAT32. `mount(2)` binds it with
fstype `fat`, on a device whose first sector is the volume's:

```sh
mkfs.fat DEVICE                      # makes a FAT32 volume
mount /dev/sda/data /mnt fat
mount /dev/sda/p1 /mnt fat           # a partition
```

`mkfs.fat` is fstool's formatter, through `fatvol::format`.

- It supports long names (VFAT). It creates, writes, cuts, grows with
  zeros, renames (across directories too) and removes files and
  directories, and sets file times. It keeps both FAT copies and the FAT32
  FSInfo up to date.
- FAT keeps no owners or permissions: everything reads as mode 0755, or
  0555 with the read-only attribute.
- A name that only a short 8.3 entry records is listed in lower case (`MSG`
  as `msg`, like Linux's `shortname=lower`). Lookups ignore case.
- The module caches the allocation table and writes it back when the last
  fd on a file closes and at `umount`.

The filesystem is the `fatvol` crate over fstool's FAT driver (crates.io,
pinned). `cargo test -p fatvol` checks it against `fsck.fat` and mtools,
and the boot test runs `fsck.fat -n` on the FAT test disk.

## ext2

The ext2 module handles the ext2 that Linux and e2fsprogs know: 1, 2 and
4 KiB blocks, block groups, indirect blocks up to triple, symlinks, rename,
sparse superblocks and files over 2 GiB. `mount(2)` binds it with fstype
`ext2`, on a disk that `mkfs.ext2` (or Linux's `mke2fs -t ext2`) formatted.

`cargo test -p ext2fs` checks it against `e2fsck` and `debugfs`. CI boots
carry an empty 4 GiB scratch disk (`/dev/nvme1n1/data`) for big
filesystems.

## Network files

The virtio-net driver gives the Ethernet device
`/dev/net0/data`. netfs mounts Plan 9 `/net`, and netd runs smoltcp in
userspace over `/dev/netd/data`. `ping <ipv4|ipv6>` (`/bin/custom/ping`)
uses `/net/icmp`. `docs/net.md` has the details.
