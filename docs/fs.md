# Filesystems and disks

myos has one file tree. The read-only image sits at `/`, and the kernel's
own filesystems (`/tmp`, `/dev`, `/proc`) and the disks' filesystems are
mounted over it. This page covers the VFS and its mounts, open files and
locks, the kernel's filesystems, disks, the caches, and the FAT and ext2
modules. The code is in `kernel/src/fs/` and `kernel/src/blk*`. The
network's files (`/dev/net0`, Plan 9 style `/net`) are in `docs/net.md`.

## The VFS and mounts

A path goes to the mount with the longest matching prefix. `/proc/mounts`
shows the table. `mount SRC TARGET bind` shows a directory or file at a
second place too.

Every path a syscall touches is checked against the security policy
(`/etc/policy`, `kernel/src/sec`) and the caller's namespace
(`kernel/src/task/ns.rs`, which also implements `chroot`). Labels come
from the canonical path, so a bind mount gives no second label
(`docs/security.md`).

### Mounting a disk

```sh
mount /dev/sda/data /mnt ext2
mount PARTUUID=<guid> /mnt ext2      # the partition's unique GUID
umount /mnt
```

- The target is any existing directory that is not a mount point yet.
  `/mnt` is `/tmp/mnt` bound over the image's, so `mkdir /mnt/usb` works.
  A disk is mounted once.
- At boot, init's `mount -a` mounts the boot disk's ESP at `/boot`, what
  its `/boot/fstab` lists, and the data partition at `/data`
  (`docs/install.md`).
- `umount` succeeds once no file on the disk is open or mapped and nothing
  is mounted below. The filesystem's `unmount` hook writes back its cache
  first.
- A mount point's directory can be neither renamed nor removed, and the
  kernel's own trees (`/`, `/tmp`, `/dev`, `/proc`) cannot be unmounted.

## Open files

An fd, a file mapping or a running program holds its file, not its name
(`kernel/src/fs/node.rs`). It follows a rename. A file unlinked or
replaced while held stays usable through the holder until the last one
lets go. tmpfs and ext2 (through the module's `unlink_keep` hook) support
this; on other filesystems the holder's reads and writes fail.

Once open, a file on a filesystem with file ids (tmpfs, ext2's inode
numbers through the `file_id` hook) is reached by its id, which is also
`stat`'s `st_ino`.

There are no hard links, on purpose: a file has one name
(`docs/security.md`), and `link` fails with `EPERM`.

## File locks

File locks (`kernel/src/fs/lock.rs`) are advisory, as on Unix. There are
two kinds, and they never meet:

- `flock` locks belong to the open file description: shared by `dup` and
  `fork`, released with its last fd.
- `fcntl` record locks (`F_SETLK`, `F_SETLKW`, `F_GETLK`, `lockf`) lock
  byte ranges for the process. As POSIX says, they go when the process
  exits or closes any fd on the file. Linux's open-file-description record
  locks belong to the description instead.

A conflicting request fails with `EWOULDBLOCK` or waits. A caught signal
ends the wait (`EINTR`). The syscalls are in `docs/userspace.md`.

## The kernel's filesystems

### rootfs

rootfs is the read-only tree at `/` (`kernel/src/fs/rootfs.rs`). Its files
come from:

- the initramfs, a zstd-compressed cpio archive Limine loads next to the
  kernel. It carries the kernel modules too (`/lib/modules`,
  `docs/modules.md`). The kernel decodes it once at boot and unpacks it
  without a copy (`kernel/src/fs/zstd.rs`);
- the tests module, on the build's disks (`docs/testing.md`);
- a module's `vfs_register`, which adds or replaces a file at run time.

The kernel embeds no program: it starts `/bin/custom/init` from the
initramfs. A directory exists through its files, so an empty one carries a
`.keep` file (`mnt/.keep`).

### tmpfs and devfs

tmpfs is the writable mount at `/tmp`. Its files live in the kernel heap,
up to 128 MiB per file (issue #338). devfs at `/dev` holds the device
nodes.

### procfs

procfs files are generated when read. `docs/proc.md` covers the process,
CPU and memory files. The others:

| File | What |
|------|------|
| `/proc/mounts` | the mount table |
| `/proc/partitions` | the disks' partitions (below) |
| `/proc/cmdline` | Limine's `cmdline:`, which names the boot slot |
| `/proc/boot/kernel`, `/proc/boot/initramfs`, `/proc/boot/partuuid` | the files and the partition this boot came from |
| `/proc/self/ctx` | the caller's uid, user and domain |
| `/proc/sys/security/users` | the policy's users, what `getpwnam` reads |
| `/proc/sys/kernel/hostname` | the host name ("myos" at boot), writable, up to 64 bytes |
| `/proc/sys/kernel/overflowuid`, `overflowgid` | 65534 (what bubblewrap reads) |

## Disks and partitions

The disk drivers are modules that register their disks through
`blk_register` (`docs/modules.md`):

| Driver | Disks |
|--------|-------|
| virtio-blk | `vda`, `vdb`, ... |
| virtio-scsi, USB storage | `sda`, `sdb`, ... |
| NVMe | `nvme0n1`, ... |
| SD | `mmcblk0` |

A disk is a directory, `/dev/<disk>/` (`kernel/src/blk.rs`,
`kernel/src/blk/gpt.rs`):

- `data` is the whole disk.
- `p<N>` is the partition in entry `N` of its GUID partition table (a gap
  stays a gap). A partition is a block device confined to its sectors.
- `info` is one line: the disk's name, its size in bytes, its driver and
  where the disk is, and what the device reports itself to be.
  `cat /dev/*/info` shows all of them:

```
nvme0n1 1610612736 nvme pci 00:04.0 model "QEMU NVMe Ctrl" serial "myos"
sdb 67108864 usb_storage usb 0x002 46f4:0001 vendor "QEMU" model "QEMU HARDDISK"
```

The kernel reads a disk's GPT on the first look at `/dev` after it
appeared, falling back to the backup table when the primary is bad. MBR
tables are not read. `/proc/partitions` lists the partitions: first
sector, bytes, type GUID, unique GUID and name.

```
nvme1n1/p1 2048 20971520 c12a7328-f81f-11d2-ba4b-00a0c93ec93b <unique GUID> "EFI system"
```

A removed disk (a USB stick pulled out) takes its partitions with it; the
kernel refuses that while one is mounted or open. Writing `rescan` to
`/proc/pci` rereads the tables of the disks none of whose partitions is in
use.

## Caches and shared memory

- **Block cache.** Disk reads stay in 4 KiB pages, up to an eighth of RAM,
  least recently used out first, all given back when memory runs out.
  Writes go through to the disk. `BlockCacheKiB` in `/proc/meminfo`.
- **Page cache** (`kernel/src/fs/pagecache.rs`). A file's pages mapped
  without write permission (programs' and libraries' code and constants)
  are one shared frame each, kept for the next process. A write,
  truncation or unlink drops them. Up to a quarter of RAM
  (`PageCacheKiB`).
- **Shared file mappings.** A `MAP_SHARED` mapping of a regular file
  through a writable fd is the file itself: its pages are the cache's
  frames, `read` and `write` see changes at once, and `msync`, `munmap`,
  exit and exec write them back.
- **Shared memory.** A `MAP_SHARED | MAP_ANON` mapping is a nameless
  tmpfs file. POSIX (`shm_open`, `memfd_create`) and System V (`shmget`,
  ...) shared memory are tmpfs files in `/dev/shm`; a System V segment's
  id is its inode number. A segment costs its size twice (the mapped
  frames and the tmpfs bytes), up to tmpfs's 128 MiB cap (issue #338).
- **Fork is copy-on-write** (`kernel/src/user/cow.rs`). A forked child
  shares its parent's private pages read-only, and the first store by
  either side copies the page. Shared mappings and device pages stay
  shared. `Forks` and `CowCopies` in `/proc/meminfo` count them.

## FAT

The FAT module (`modules/fat`) reads and writes FAT16 and FAT32 with long
names:

```sh
mkfs.fat DEVICE                      # makes a FAT32 volume
mount /dev/sda/data /mnt fat
mount /dev/sda/p1 /mnt fat           # a partition
```

- FAT keeps no owners or permissions: everything reads as mode 0755, or
  0555 with the read-only attribute.
- A name only an 8.3 entry records is listed in lower case, like Linux's
  `shortname=lower`. Lookups ignore case.
- The module writes its cached allocation table back when the last fd on
  a file closes and at `umount`.

The filesystem is the `fatvol` crate over fstool's FAT driver (crates.io,
pinned); `mkfs.fat` is fstool's formatter. `cargo test -p fatvol` checks it
against `fsck.fat` and mtools, and the boot test runs `fsck.fat -n` on the
FAT test disk.

## ext2

The ext2 module (`modules/ext2`, the `ext2fs` crate) handles the ext2 that
Linux and e2fsprogs know: 1, 2 and 4 KiB blocks, indirect blocks up to
triple, symlinks, sparse superblocks and files over 2 GiB. Format with
`mkfs.ext2` (or Linux's `mke2fs -t ext2`) and mount with fstype `ext2`.

`cargo test -p ext2fs` checks it against `e2fsck` and `debugfs`. CI boots
carry an empty 4 GiB scratch disk (`/dev/nvme1n1/data`) for big
filesystems.
