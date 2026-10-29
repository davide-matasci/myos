# Kernel modules

## Overview

Kernel modules are ELFs in RAM. One loader copies `PT_LOAD`, applies relocs, calls `module_init`. The kernel embeds none of them:

| | Boot | Runtime (`insmod`) |
|---|---|---|
| Bytes live in | `/lib/modules/<name>` in the initramfs, named in load order by `/lib/modules/boot.list` | `/lib/modules/<name>` in the initramfs (or any file) |
| Loaded by | `modules::load_boot_modules` right after the initramfs is unpacked | `SYS_INSMOD` from `/bin/custom/insmod` |

`/proc/modules` lists what is loaded. `rmmod <name>` (`SYS_RMMOD`) unloads a module that provides nothing any more: the kernel counts what each module registered through the `KernelApi` (devices, filesystems, mounts, `/proc` nodes, interrupts, the console, a personality, a service, a thread, a service it looked up) and refuses to unload one with a registration left; only block devices unregister (`blk_unregister`, a USB stick pulled out); `hello` unloads, a driver does not. Writing `rescan` to `/proc/pci` re-enumerates the bus and then calls every module's `module_rescan`: the block drivers bring up the controllers and disks that appeared since boot (`/dev/nvme1n1/data`, `/dev/vdb/data`, ...) and leave the known ones alone. The console module goes first (it paints the buffered boot output), then hello, pci_enum, acpi, rpi_firmware (the Pi's firmware, for the drivers that ask it), the block drivers (virtio_blk, virtio_scsi, nvme, sdmmc), the USB bus (usb, the core, the controller drivers xhci and dwc2, then the class drivers usb_hub, usb_storage and usb_hid), virtio_net, tap, netfs and the filesystems (fat, ext2). Modules behind a Cargo feature (`OPTIONAL_MODULES`: `linux` with `linux_compat`) are always shipped under `/lib/modules` but only listed in `boot.list` when the feature is on; `pcidev` and `pl061` (`RUNTIME_MODULES`) are shipped and never listed: root loads them (`docs/pci-acpi-smp.md`). The boot drive holds the kernel and the initramfs only, so an upgrade replaces two files.

Module exports:
```rust
unsafe extern "C" fn module_init(api: *const KernelApi) -> i32
unsafe extern "C" fn module_exit()   // optional: run by rmmod
unsafe extern "C" fn module_rescan() // optional: probe for new devices after a /proc/pci rescan
```

`KernelApi` (`modules/abi`) is a `#[repr(C)]` table, ABI v59 (v59 gives a module a PCI function's I/O BAR, `pci_io_map`, and a device of 32-bit pointers DMA it reaches, `DmaDevice::below`, `docs/pci-acpi-smp.md`; v56 gives a board device's interrupt back, `irq_disable`, the line masked once no handler is left on it, and an interrupt domain's `mask` beside its `unmask`; v55 gives a PCI function's interrupt back, `pci_irq_disable`: its vector, handler slot and IOMMU remapping entry free for the next device, `docs/pci-acpi-smp.md`; v54 hands the kernel the PCI interrupt routes the firmware's `_PRT` gives, `pci_intx_route`, which x86_64 routes a function's INTx by, and maps physical memory for the acpi module's AML interpreter, `phys_map`, `docs/pci-acpi-smp.md` "AML"; v52 gives `/dev/pci` a BAR's address as the CPUs see it, `pci_bar_phys`, whether a device's DMA sees the caches, `dma_coherent`, and non-cacheable mappings for one that does not, `MYOS_MMAP_NOCACHE`, `docs/pci-acpi-smp.md`; v51 adds `irq_domain_register` / `irq_dispatch`, a module's secondary interrupt controller as an interrupt domain, and `MmioDevice::phandle`; v50 lets a file's `open` hook name another file for the fd: opening a `/net` protocol's `clone` gives the new conversation's `data`, `docs/sockets-curl.md`; v45 lets a filesystem's write hooks and `vfs_write` report a full filesystem, `VFS_ENOSPC`, the program's `ENOSPC`; v44 gave the `poll` hooks a change count, what an edge-triggered wait set member is reported against, `docs/events.md`; v35 added a filesystem's `statfs` hook and `vfs_statfs`: how full it is; v34 made a filesystem's stat size 64 bits, files and disks past 4 GiB; v33 added `irq_enable` (a board device's interrupt: a GIC SPI, a PLIC source, an ISA IRQ through the I/O APIC) and `console_input`, and `keyboard_irq` / `cursor_blinks` to the console's ops: the keyboards interrupt, `docs/pci-acpi-smp.md`; v32 added `current_uid` (the user the calling process runs for, `docs/security.md`); v31 added `power_register` (a module's power-off or reboot method, `docs/power.md`); v30 added a filesystem's `file_id` and `set_times_ino` hooks (the `*_ino` hooks are how an open file is reached) and `fd_lockctl` (file locks for the Linux layer); v29 added a filesystem's `set_size` and `set_size_ino` hooks (`ftruncate`); v28 added the hooks that keep a file unlinked while it is held (`unlink_keep`, `read_ino`, `write_ino`, `stat_ino`, `forget_ino`); v27 added `thread_place`; v26 added a filesystem's `unmount` hook (`umount`); v25 added file times, a filesystem's `set_times` hook and `vfs_set_times`; v21 added `blk_unregister`, the service registry (`service_register` / `service_lookup`) modules reach each other through, module threads (`thread_spawn`), `wake` and the USB bus types, `docs/usb.md`; v19 took `ioctl` out, v20 added the `open` hook that makes a file exclusive, `/dev/console/kbd`). Kernel fills it and passes it to `module_init`. Drivers register what they provide: `blk_register` (block devices), `dev_register` (char devices: the directory `/dev/<name>/` with `data` and an optional text `ctl`, and a `poll` hook for `data`'s readiness), `fs_register` / `vfs_mount` (filesystems; a backend's optional `mmap` hook maps device memory, `docs/fb.md`, and its `poll` hook reports readiness for `poll`), `console_register` (screen + keyboard), `personality_register` (a foreign syscall ABI, see `docs/linux-compat.md`); `dt_mmio_find` gives a driver its memory-mapped devices from the device tree.

A module calls the table through its safe methods, one per entry with the
same name (`api.blk_read(dev, lba, &mut buf)` for
`(api.blk_read)(dev, lba, ptr, len)`): they take slices, `&str` and
references, and the kernel copies or checks what it is handed, so no
`unsafe` is needed. The few entries that cannot be safe (`dealloc`, the
saved registers of a syscall, the FP/SIMD save area) have no method. A
module keeps the table `module_init` received in a `static API: ApiCell`
(`API.get()` afterwards) and its own state behind a `myos_abi::Lock`, a
spin lock, or a `SleepLock` when it is held across a wait (a block driver's
disks are one lock per slot, held for a request; usb_storage's requests
and the USB host's transfers wait for their completion, so those sleep).
No module has a `static mut`; what stays `unsafe` is hardware access
(MMIO, DMA rings, cache maintenance) and the raw pointers of the C ABI's
callbacks, in short helpers with `SAFETY:` comments. Every module crate
denies `unsafe_op_in_unsafe_fn`.

### Adding a module
1. Copy `modules/hello` → `modules/foo` (keep panic=abort, opt-level=s, myos-abi, link flags)
2. Add it to the module list in `kernel/build.rs` (builds `target/foo-<triple>` for every arch) and to `BOOT_MODULES` in `src/limine_image.rs` at the position it must load (that ships it in the initramfs and puts it in `/lib/modules/boot.list`); list the ELFs in `scripts/ci-build-kernels.sh` / `ci-pack-build-artifacts.sh`
3. Or skip the boot list and load it on demand: `insmod /lib/modules/foo`
