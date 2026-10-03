# TODO

Follow-ups left open after the blocking scheduler (#187), virtio-net RX
interrupts (#188) and the modular kernel (arch/ split, process blocks, every
driver a module). Roughly in priority order.

## x86_64 interrupt routing beyond MSI-X

aarch64 and riscv64 take their PCI INTx routing, controller bases and
virtio-mmio devices from the device tree (`kernel/src/dt.rs`). On x86_64
MSI-X is the primary path and portable; INTx would need `_PRT` from the DSDT
plus an IOAPIC driver, neither of which exists (the ACPI module only scans
for `_S5`). Interrupt remapping (IOMMU) and x2APIC destinations above 255
CPUs are also unhandled. A device without MSI-X today degrades to netd's 1 s
`NET_WAIT_RX` backstop rather than breaking.

## Device tree: what is still assumed

The tree gives the GIC / PLIC, UART, RTC, PCIe host bridge and `virtio,mmio`
nodes. Not read yet: the CPU list (Limine's MP bring-up enumerates CPUs),
`clint`, GICv3 (`arm,gic-v3`: redistributors and the ICC system registers
instead of the GICv2 memory-mapped CPU interface), a UART other than PL011 /
16550, and `interrupt-map` entries whose parent is not the one interrupt
controller. A board needing any of these fails at boot with a `fatal: device
tree: ...` line on the default QEMU `virt` UART address.

## Module follow-ups

- **Device rescan / hotplug**: drivers probe once in `module_init`. Writing
  `rescan` to `/proc/pci` used to re-probe the in-kernel NVMe driver; now it
  only refreshes the listing. A `rescan` hook in `ModuleBlkOps` (or a generic
  module op the pci_enum writer calls) would restore that.
- **`rmmod`**: `module_exit` exists but nothing calls it; unloading needs
  unregister paths for blk/chr/fs/console ops and a check that no fd or
  mount still uses them.
- **Per-process locks**: the `Process` blocks (`task/process.rs`) hang off
  the one `TASKS` lock. Giving each its own lock (TASKS → process ordering)
  would let fd/mmap syscalls of different processes stop contending.
- **Module dependencies**: the boot order in `BOOT_MODULES` is the only
  ordering (console first, block drivers before filesystems). A module could
  declare what it needs (`blk_count() > 0`, another module's name) so
  `insmod` can refuse or defer.

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

## Rust std `thread::sleep`

The myos `std` sysroot has no `thread::sleep`; wire it to `SYS_NANOSLEEP`
(52). Touching `toolchain/std` triggers a sysroot rebuild in CI.

## File offsets are per fd copy

A forked child (or a `dup`) gets its own copy of an open file's offset
(`FdEntry::File { pos }` in `kernel/src/task/fd.rs`), where POSIX shares
one open file description. With `prog > file`, a program whose children
write to the inherited fd then overwrites their output with its own later
writes (the `heap` smoke's log lost its first lines). The boot tests work
around it with `>>` (`O_APPEND` writes at the end), see `capture` in
`user/tests/run.sh`. The fix is an open-file table the fd entries point
into, shared across fork and dup.

## Passing file descriptors and shared memory

What a GUI needs beyond `/net/unix` (`docs/sockets-unix.md`) and
`/dev/fb0` (`docs/fb.md`). A client of a display server draws into memory
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
  of private copies (today only `/dev/fb0` does, `user::do_mmap`). For a
  tmpfs file that means pages owned by the file and refcounted by their
  mappings (tmpfs keeps a file as one contiguous buffer now), with writes
  through `write(2)` and through mappings seeing each other. Then
  `shm_open` (a tmpfs file under `/dev/shm` or `/tmp`), `memfd_create` and
  `ftruncate` on top; the Linux layer's `mmap` would stop refusing
  `MAP_SHARED`. Anonymous `MAP_SHARED` (shared across `fork`) falls out of
  the same refcounted pages.
