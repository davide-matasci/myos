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
(`packages/tinyx/README.md`; GPL-3.0, the rest MIT/X11), and
`packages/dwm` the window manager (dwm drawing with core X fonts,
`packages/dwm/README.md`). Everything is linked statically.

1. **First clients, libX11 only**: `xsetroot`, `xev`.
2. **A terminal**, the one dwm's Alt+Shift+Return starts: `st` drawing with
   core fonts like dwm (its `x.c` uses Xft throughout, a bigger patch than
   dwm's `drw.c`), or `xterm` (needs Xt, Xaw, Xmu, Xpm; termcap from
   `ports/termcap`; libX11's locale data, which x11-libs does not ship yet).
   Then `dmenu` (Alt+P), with dwm's core-font `drw.c`.
3. **xinit / startx**: start the server and a session (a window manager
   and a terminal) together.

Gaps that may show up on the way:

- `setitimer` fails (`ENOSYS`) and `alarm` is missing: the server runs its
  plain scheduler (it prints "scheduling timer: Function not implemented");
  `xterm`'s blinking will want a timer.
- `/net/unix` buffers 8 KiB per end and has 32 conversations: enough to
  start, slow for big replies (`GetImage`, large `PutImage`).
- A real `pthread_create` (on `thread_spawn` / `wait_addr`) if a client
  needs threads; the libgloss pthread functions are single-threaded.
- MIT-SHM, and with it fast image transfers, needs the shared memory below.

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

User tasks are pinned to a home CPU chosen round-robin at spawn/fork (the
pinning is what keeps TLB flushes local). An idle CPU could steal a Ready task
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
  `shm_open` (a tmpfs file under `/dev/shm` or `/tmp`), `memfd_create` and
  `ftruncate` on top; the Linux layer's `mmap` would stop refusing
  `MAP_SHARED`. Anonymous `MAP_SHARED` (shared across `fork`) falls out of
  the same refcounted pages.
