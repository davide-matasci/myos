# TODO

Follow-ups left open after the blocking scheduler (#187), virtio-net RX
interrupts (#188) and the modular kernel (arch/ split, process blocks, every
driver a module). Roughly in priority order.

## X11 as a package (minimal)

An X server and a few clients as packages (`packages/`, `get-myos`): core
protocol only, no mouse, no extensions beyond what the server cannot drop.
The screen is `/dev/fb` (`docs/fb.md`), the keyboard `/dev/console/kbd`
(`docs/tty.md`, Linux `KEY_*` codes: X keycode = code + 8), clients connect
over `/net/unix` (`/tmp/.X11-unix/X0` is a name there,
`docs/sockets-unix.md`). libc has `readv`/`writev` and a single-threaded
pthread API, `packages/x11-libs` builds the client libraries (libxcb,
libX11; `packages/x11-libs/README.md`) and `packages/tinyx` the server,
TinyX's `Xfbdev` on `/dev/fb` and `/dev/console/kbd`
(`packages/tinyx/README.md`; GPL-3.0, the rest MIT/X11),
`packages/x11-xft` the client-side fonts (FreeType, fontconfig, Xft) with
`packages/x11-fonts` (DejaVu Sans Mono), and `packages/dwm` the window
manager, unpatched (`packages/dwm/README.md`), with what its keys start:
`packages/st` the terminal and `packages/dmenu` the menu
(`packages/st/README.md`, `packages/dmenu/README.md`). Everything is linked
statically.

1. **First clients, libX11 only**: `xsetroot`, `xev`.
2. More fonts (a proportional DejaVu Sans) when a client wants them.
3. **A session file**: `startx` (`packages/tinyx`) runs the server and one
   client (dwm); a `~/.xinitrc`-like script to start a terminal next to the
   window manager.

Gaps that may show up on the way:

- `setitimer` fails (`ENOSYS`) and `alarm` is missing: the server runs its
  plain scheduler (it prints "scheduling timer: Function not implemented");
  `xterm`'s blinking will want a timer.
- `/net/unix` buffers 8 KiB per end and has 32 conversations: enough to
  start, slow for big replies (`GetImage`, large `PutImage`).
- A real `pthread_create` (on `thread_spawn` / `wait_addr`) if a client
  needs threads; the libgloss pthread functions are single-threaded.
- MIT-SHM, and with it fast image transfers, needs the shared memory below.
- oksh sets `PATH` to its default whatever it inherits
  (`ports/oksh/main.myos.patch`), so `PATH=~/bin:$PATH dmenu_run` (or any
  script) sees the default. The override keeps SSH logins working (dropbear
  passes `/usr/sbin:/usr/bin:/sbin:/bin`): respecting an inherited `PATH`
  wants dropbear's `DEFAULT_ROOT_PATH` set to myos's directories first.
- libgloss's `setsid` only starts a process group (`SYS_SETSID` corrupted
  netfs writes, not root-caused): sessions are approximated along the
  parent chain (`kernel/src/pty.rs`).

## File primitives, part 2 (one PR)

What libc still fakes about files after `O_EXCL`, `ftruncate`, close-on-exec
and `pread`/`pwrite` became real (`README.md`, "File I/O"):

1. **Hard links**: `linkat` fails with `EROFS` (libgloss `more_stubs.c`).
   ext2 links natively (an inode's link count, which `unlink_keep` and
   `forget` already honour); tmpfs keeps a file under its one name, so it
   needs its entries split into names and files (a refcounted file a name
   points to) first. The node table (`kernel/src/fs/node.rs`) maps a node
   to one location: two names of one file are two nodes unless it is keyed
   by the file (mount + inode) rather than its path. A new `linkat`
   syscall and a module hook (ABI append), `st_nlink` from the filesystem.
2. **File locks**: `flock` is `ENOSYS`, `fcntl` record locks
   (`F_SETLK`/`F_SETLKW`/`F_GETLK`) do not exist, and the Linux layer grants
   both without keeping them. A kernel lock table keyed by node: `flock`
   locks owned by the open file description, record locks by the process,
   released on the last close and at exit, `F_SETLKW` and `LOCK_EX` without
   `LOCK_NB` waiting (interruptible) on the node. git, vim's swap files,
   SQLite and cargo use them.
3. **`O_NOFOLLOW` and `O_DIRECTORY`** on `openat`: refuse a symlink in the
   last component (`ELOOP`), or anything but a directory (`ENOTDIR`). The
   resolution already knows (`resolve(.., follow)`, `fs::resolve_beneath`);
   the Linux layer strips both today. With them, `opendir` and the
   `*at` walks of a tree (`rm -r`, `find`) are safe against a symlink
   swapped in mid-walk.

## Filesystems on inodes and shared file mappings (one PR)

1. **An inode interface for filesystems**: every hook of `MountOps` and
   `ModuleVfsOps` takes a path relative to the mount, so each read and
   write is a lookup from the mount's root inside the filesystem (ext2
   caches the last resolutions, `Fs::resolved`); only files kept past an
   unlink have inode calls (`read_ino`, ...). Hooks on a file id the
   filesystem hands out at lookup (ext2's inode number, a tmpfs file's
   index) for `read`, `write`, `stat`, `set_size`, `set_times`, the path
   ones only for lookup and the namespace changes (`create`, `unlink`,
   `rename`, ...). The node table keeps the id next to the location; the
   `*_ino` hooks become the normal ones. An ABI break for the filesystem
   modules (fat, ext2, netfs, console's devices).
2. **`MAP_SHARED` of regular files**: a shared mapping works only for a
   device's memory (a module's `mmap` hook, `/dev/fb/data`). Pages owned
   by the file and refcounted by the mappings that use them, `write(2)`
   and the mappings seeing each other, written back on `msync`, `munmap`
   and the last close; the Linux layer's `mmap` stops refusing it. The
   page cache (`kernel/src/fs/pagecache.rs`) has the shared, counted
   frames; what is missing is writing through them (it drops a file's
   pages on a write today). The same pages give the shared memory of
   "Passing file descriptors and shared memory" (`shm_open`,
   `memfd_create`, sized with `ftruncate`).

## Self-hosting speed

Building core+alloc inside myos (the first step of `linux-compat/self-host.sh`)
takes ~1250 s under TCG with `-smp 4` (it varies by ±15% between runs on a
shared host); Alpine's Linux takes 591 s in the same QEMU (41 s natively).

QEMU's `info jit` showed myos making QEMU translate more code and
invalidate translated code far more often than Linux (QEMU keeps translated
code by physical address, and myos copied every file page into a fresh
frame per process). The page cache (`kernel/src/fs/pagecache.rs`) fixed
that: 30x fewer invalidations, 21% less translated code, no flush of
QEMU's code buffer. The build did not get faster: translation was not
the bottleneck. The build runs mostly on one vCPU (15 min of its CPU in
both kernels), so the gap to Linux is in how fast that vCPU runs rustc.
Not yet measured:

- **The 1 kHz tick**: ~2000 schedules a second on every CPU, idle ones
  too (each takes the scheduler lock). Every interrupt makes QEMU leave its
  translated code; Linux ticks at 100-250 Hz and not at all when idle, and
  a tickless idle CPU would leave QEMU's vCPU thread asleep.
- **TLB flushes**: ~1.1M partial flushes per build (each costs QEMU its
  softmmu TLB entries for the page, then refills).

What the page cache leaves open:

- **Copy-on-write private pages**: a writable private mapping gets a copy
  of the file page at the first fault, read or write; mapping the cached
  frame read-only until the first write would share data pages too.
- **`/proc/self/exe`**: missing; an `$ORIGIN` rpath (rustc's) works only
  because musl takes ENOENT from it as "no origin".

Smaller, measured on the way:

- **fork copies everything**: `fork` (and posix_spawn's `fork_from`) copies
  the whole image, heap and touched mmap pages at once; cargo spawning
  rustc copies cargo each time. Copy-on-write, or a spawn that maps nothing
  of the parent's, would make it cheap.
- **Fault-around**: a fault maps one page; mapping the cached neighbours of
  a file page at once (Linux maps 16) cuts the 900k faults of the build.
- **NVMe moves a page per command** (`modules/nvme`: one PRP); with a PRP
  list a run of blocks is one command, and the block cache could read
  ahead.

## x86_64 interrupt routing beyond MSI-X

aarch64 and riscv64 take their PCI INTx routing, controller bases and
virtio-mmio devices from the device tree (`kernel/src/dt.rs`). On x86_64
MSI-X is the primary path and portable; INTx would need `_PRT` from the DSDT
plus an IOAPIC driver, neither of which exists (the ACPI module only scans
for `_S5`). Interrupt remapping (IOMMU) and x2APIC destinations above 255
CPUs are also unhandled. A device without MSI-X today degrades to netd's 1 s
`NET_WAIT_RX` backstop rather than breaking.

## Device tree: what is still assumed

The tree gives the GIC (v2 or v3) / PLIC, UART (PL011, 16550 with its
`reg-shift`), RTC, PCIe host bridge and `virtio,mmio` nodes. Not read yet:
`clint`, a UART of another kind, the GICv3 ITS (PCI devices use INTx on
aarch64), and `interrupt-map` entries whose parent is not the one interrupt
controller. A board needing any of these fails at boot with a `fatal:
platform: ...` line on the default QEMU `virt` UART address.

## Module follow-ups

- **Unregister paths**: `rmmod` unloads a module only while it provides
  nothing (the kernel counts its registrations and refuses otherwise).
  Block devices unregister (`blk_unregister`, refused while mounted or
  open: a USB stick pulled out). To unload a driver or a filesystem, the
  chr/fs/console/personality registries need the same and a check that no
  fd, mount or task still uses them.
- **Hotplug notification**: a hot-added device appears after `rescan` is
  written to `/proc/pci` (`module_rescan`); no ACPI/QEMU hotplug interrupt
  triggers that by itself. virtio-net probes once (netd binds the one
  `/dev/net0`).
- **Per-process locks**: the `Process` blocks (`task/process.rs`) hang off
  the one `TASKS` lock. Giving each its own lock (TASKS → process ordering)
  would let fd/mmap syscalls of different processes stop contending.
- **Module dependencies**: the boot order in `BOOT_MODULES` is the only
  ordering (console first, block drivers before filesystems). No module
  needs another at init today (the filesystems register a type and read
  their disk at mount time), so nothing declares dependencies yet; a module
  that does could name them for `insmod` to refuse or defer.

## Per-key wait queues

`task::wake(key)` takes the global task lock and scans all 64 slots to find
the tasks blocked on that key. Fine at today's task counts; a list of waiters
per object (pipe, pty, child, console) would make wakes O(waiters) and keep
the scan out of the hot path once the task table grows.

## Idle-pull load balancing

User tasks are pinned to a home CPU chosen round-robin at spawn/fork, and
threads at their creation (an address space loaded on several CPUs is
flushed on all of them: `user::flush_user_tlb`). An idle CPU could steal a Ready task
whose home CPU is busy; needs a cross-CPU TLB shootdown on migration and the
NX #PF / leave races noted in `docs/pci-acpi-smp.md` resolved first.

## Passing file descriptors and shared memory

What a GUI needs beyond `/net/unix` (`docs/sockets-unix.md`) and
`/dev/fb` (`docs/fb.md`). A client of a display server draws into memory
the server can read without copying it through a socket, and Wayland's core
protocol is built on that: the client creates a shared-memory file and sends
its fd to the compositor. X does without, but its MIT-SHM extension (and so
the speed of every image-heavy client) needs the same.

- **fd passing**: `sendmsg`/`recvmsg` with `SCM_RIGHTS` on `/net/unix`
  connections. The sender's open file (`FdEntry`) is taken a reference on
  and queued with the bytes it travels with; the receiver gets a new fd for
  it when it reads past that point. Needs a kernel path from netfs to the
  fd tables (a `KernelApi` call, append-only), care with the open-ref
  counts (`fs::vfs::open_ref`) and with fds still in flight when either end
  closes. `SCM_CREDENTIALS` / `SO_PEERCRED` (the peer's pid and uid) come
  cheaply along with it.
- **Shared memory**: `mmap(MAP_SHARED)` of a file maps its pages instead
  of private copies (today only a device's, through a module's `mmap`
  hook: `/dev/fb/data`, `user::do_mmap`). For a tmpfs file that means pages
  owned by the file and refcounted by their mappings (tmpfs keeps a file as one contiguous buffer now), with writes
  through `write(2)` and through mappings seeing each other. Then
  `shm_open` (a tmpfs file under `/dev/shm` or `/tmp`) and `memfd_create`
  on top, sized with `ftruncate`; the Linux layer's `mmap` would stop refusing
  `MAP_SHARED`. Anonymous `MAP_SHARED` (shared across `fork`) falls out of
  the same refcounted pages.
