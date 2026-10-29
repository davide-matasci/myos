# Kernel modules

Every driver and filesystem in myos is a kernel module: an ELF the kernel
loads into RAM. The kernel embeds none of them. This page covers loading
and unloading, the `KernelApi` table modules call, and how to add one.

## Loading and unloading

One loader (`kernel/src/modules/`) copies the `PT_LOAD` segments, applies
the relocations and calls `module_init`. A module's bytes are
`/lib/modules/<name>` in the initramfs, so the boot drive holds the kernel
and the initramfs only, and an upgrade replaces two files.

- At boot, `modules::load_boot_modules` loads the modules
  `/lib/modules/boot.list` names, in order, right after the initramfs is
  unpacked.
- At run time, `insmod /lib/modules/foo` (`SYS_INSMOD`) loads a module
  from any file.
- `/proc/modules` lists what is loaded.
- `rmmod <name>` (`SYS_RMMOD`) unloads a module that provides nothing any
  more. The kernel counts what each module registered through the
  `KernelApi` (devices, filesystems, mounts, `/proc` nodes, interrupts,
  services, threads, ...) and refuses while a registration is left. Only
  block devices unregister (a USB stick pulled out), so `hello` unloads and
  a driver does not.
- Writing `rescan` to `/proc/pci` re-enumerates the bus, then calls every
  module's `module_rescan`. The block drivers bring up the controllers and
  disks that appeared since boot and leave the known ones alone. There is
  no ACPI or QEMU hotplug interrupt yet, so a hot-added disk shows up only
  after a rescan. virtio-net probes once, at load (netd binds the one
  `/dev/net0`).

### Boot order

`BOOT_MODULES` in `src/limine_image.rs` sets the order. `console` comes
first, since it paints the buffered boot output. Then come the platform
modules (`pci_enum`, `acpi`, `rpi_firmware`), the block drivers, sound,
the USB core with its controller and class drivers, the network modules,
and last the filesystems (`fat`, `ext2`), so the disks exist before them.

Two more lists ship modules under `/lib/modules` without loading them at
boot by default:

- `OPTIONAL_MODULES` are behind a Cargo feature (`linux` with
  `linux_compat`). They are always shipped, and listed in `boot.list` only
  when the feature is on.
- `RUNTIME_MODULES` (`pcidev`, `pl061`) are shipped and never listed: root
  loads them (`docs/pci.md`).

## What stays in the kernel

The kernel keeps what must run before any module, or what every module
needs.

| Component | Location | Why |
|-----------|----------|-----|
| PCI config access (`cfg_read32`, BAR map, `pci_find`) | `kernel/src/pci.rs`, `arch/*/pci.rs` | The module ABI exposes it; every PCI driver is a module |
| The IOMMU | `kernel/src/iommu` | What a device's DMA reaches (`docs/iommu.md`) |
| Platform description (ACPI static tables, device tree) | `kernel/src/platform.rs`, `acpi.rs`, `dt.rs` | Device bases, interrupt routing and clocks are needed before the heap exists (`docs/platform.md`) |
| Limine requests | `kernel/src/limine_boot.rs` | Boot protocol |
| AP bring-up, `/proc/cpuinfo` | `kernel/src/smp.rs` | Must run before modules; owns CPU-local state and IPIs |
| Scheduler | `kernel/src/task/` | Per-CPU run queues and current task |
| Serial console | `kernel/src/console.rs`, `arch/*/serial.rs` | Boot output before the console module; the module registers screen and keyboard ops |
| Built-in `/proc` files | `kernel/src/fs/procfs.rs` | `mounts`, `cpuinfo`, `platform`; modules add theirs with `proc_register` |
| Which tasks run a syscall personality | `kernel/src/personality.rs` | The Linux layer itself is `modules/linux` |

Everything else is a module under `modules/`; `docs/layout.md` lists them.

## Module exports

```rust
unsafe extern "C" fn module_init(api: *const KernelApi) -> i32
unsafe extern "C" fn module_exit()   // optional: run by rmmod
unsafe extern "C" fn module_rescan() // optional: probe for new devices after a /proc/pci rescan
```

## The `KernelApi`

`KernelApi` (`modules/abi`) is a `#[repr(C)]` table. The kernel fills it
and passes it to `module_init`. Drivers register what they provide:

| Call | What |
|------|------|
| `blk_register` | block devices |
| `dev_register` | char devices: `/dev/<name>/` with `data`, an optional text `ctl`, and a `poll` hook |
| `fs_register` / `vfs_mount` | filesystems, with optional `mmap` (`docs/fb.md`) and `poll` hooks |
| `console_register` | screen and keyboard |
| `personality_register` | a foreign syscall ABI (`docs/linux-compat.md`) |
| `service_register` / `service_lookup` | how modules reach each other |

`dt_mmio_find` gives a driver its memory-mapped devices from the device
tree.

### Calling it safely

A module calls the table through its safe methods, one per entry with the
same name: `api.blk_read(dev, lba, &mut buf)`. They take slices, `&str` and
references, and the kernel checks what it is handed, so no `unsafe` is
needed. The few entries that cannot be safe (`dealloc`, a syscall's saved
registers, the FP/SIMD save area) have no method.

A module keeps the table in a `static API: ApiCell` and its own state
behind a `myos_abi::Lock`: a spin lock, or a `SleepLock` when the lock is
held across a wait (a disk request, a USB transfer). No module has a
`static mut`. What stays `unsafe` is hardware access (MMIO, DMA rings,
cache maintenance) and the C ABI's raw callback pointers, in short helpers
with `SAFETY:` comments. Every module crate denies
`unsafe_op_in_unsafe_fn`.

### Changing the ABI

The kernel and every module are built from this tree together, so an ABI
change is made the right way (widen the field, change the struct or the
call), never with a compatibility shim. Bump `ABI_VERSION` in
`modules/abi/src/lib.rs` on every change to the table's layout or meaning,
and add a line for the new version to its doc comment, which lists what
each version changed.

## Adding a module

1. Copy `modules/hello` to `modules/foo`. Keep `panic=abort`,
   `opt-level=s`, `myos-abi` and the link flags.
2. Add it to the module list in `kernel/build.rs`, which builds
   `target/foo-<triple>` for every arch.
3. Add it to `BOOT_MODULES` in `src/limine_image.rs` at the position it
   must load. That ships it in the initramfs and puts it in
   `/lib/modules/boot.list`.
4. List the ELFs in `scripts/ci-build-kernels.sh` and
   `scripts/ci-pack-build-artifacts.sh`.

To load it on demand instead, skip the boot list and run
`insmod /lib/modules/foo`.
