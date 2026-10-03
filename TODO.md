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

- **Unregister paths**: `rmmod` unloads a module only while it provides
  nothing (the kernel counts its registrations and refuses otherwise). To
  unload a driver or a filesystem, the blk/chr/fs/console/personality
  registries need unregister paths and a check that no fd, mount or task
  still uses them.
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
