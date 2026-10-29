# Kernel modules

Every driver and filesystem in myos is a kernel module: an ELF the kernel
loads into RAM. The kernel embeds none of them. This page covers how
modules are loaded and unloaded, the `KernelApi` table they call, and how
to add one.

## Loading and unloading

One loader copies the `PT_LOAD` segments, applies the relocations and calls
`module_init`. A module is loaded at boot or at run time:

| | Boot | Runtime (`insmod`) |
|---|---|---|
| Bytes live in | `/lib/modules/<name>` in the initramfs, named in load order by `/lib/modules/boot.list` | `/lib/modules/<name>` in the initramfs (or any file) |
| Loaded by | `modules::load_boot_modules` right after the initramfs is unpacked | `SYS_INSMOD` from `/bin/custom/insmod` |

The boot drive holds the kernel and the initramfs only, so an upgrade
replaces two files.

- `/proc/modules` lists what is loaded.
- `rmmod <name>` (`SYS_RMMOD`) unloads a module that provides nothing any
  more. The kernel counts what each module registered through the
  `KernelApi`: devices, filesystems, mounts, `/proc` nodes, interrupts,
  the console, a personality, a service, a thread, a service it looked
  up. It refuses to unload a module with a registration left. Only block
  devices unregister (`blk_unregister`, a USB stick pulled out), so
  `hello` unloads and a driver does not.
- Writing `rescan` to `/proc/pci` re-enumerates the bus and then calls
  every module's `module_rescan`. The block drivers bring up the
  controllers and disks that appeared since boot (`/dev/nvme1n1/data`,
  `/dev/vdb/data`, ...) and leave the known ones alone.

### Boot order

`BOOT_MODULES` in `src/limine_image.rs` sets the order:

1. `console` first: it paints the buffered boot output.
2. `hello`, `pci_enum`, `acpi`, and `rpi_firmware` (the Pi's firmware, for
   the drivers that ask it).
3. The block drivers: `virtio_blk`, `virtio_scsi`, `nvme`, `sdmmc`.
4. Sound: `sound` and `virtio_sound` (`docs/sound.md`).
5. The USB bus (`docs/usb.md`): `usb` (the core), the controller drivers
   `xhci`, `ehci`, `uhci`, `ohci` and `dwc2`, then the class drivers
   `usb_hub`, `usb_storage`, `usb_hid`, `usb_audio` and `usbdev`.
6. `virtio_net`, `tap` and `netfs`.
7. The filesystems: `fat`, `ext2`.

Two more lists ship modules under `/lib/modules` without loading them at
boot by default:

- `OPTIONAL_MODULES` are behind a Cargo feature (`linux` with
  `linux_compat`). They are always shipped, and listed in `boot.list` only
  when the feature is on.
- `RUNTIME_MODULES` (`pcidev`, `pl061`) are shipped and never listed: root
  loads them (`docs/pci.md`).

## Module exports

```rust
unsafe extern "C" fn module_init(api: *const KernelApi) -> i32
unsafe extern "C" fn module_exit()   // optional: run by rmmod
unsafe extern "C" fn module_rescan() // optional: probe for new devices after a /proc/pci rescan
```

## The `KernelApi`

`KernelApi` (`modules/abi`) is a `#[repr(C)]` table, ABI v60. The kernel
fills it and passes it to `module_init`.

Drivers register what they provide:

| Call | What |
|------|------|
| `blk_register` | block devices |
| `dev_register` | char devices: the directory `/dev/<name>/` with `data`, an optional text `ctl`, and a `poll` hook for `data`'s readiness |
| `fs_register` / `vfs_mount` | filesystems. A backend's optional `mmap` hook maps device memory (`docs/fb.md`), and its `poll` hook reports readiness for `poll` |
| `console_register` | screen and keyboard |
| `personality_register` | a foreign syscall ABI (`docs/linux-compat.md`) |

`dt_mmio_find` gives a driver its memory-mapped devices from the device
tree.

### Calling it safely

A module calls the table through its safe methods, one per entry with the
same name: `api.blk_read(dev, lba, &mut buf)` for
`(api.blk_read)(dev, lba, ptr, len)`. The methods take slices, `&str` and
references, and the kernel copies or checks what it is handed, so no
`unsafe` is needed. The few entries that cannot be safe (`dealloc`, the
saved registers of a syscall, the FP/SIMD save area) have no method.

A module keeps the table `module_init` received in a
`static API: ApiCell` and reaches it with `API.get()` afterwards. It keeps
its own state behind a `myos_abi::Lock`:

- a spin lock;
- or a `SleepLock` when the lock is held across a wait. A block driver's
  disks are one lock per slot, held for a request. usb_storage's requests
  and the USB host's transfers wait for their completion, so those sleep.

No module has a `static mut`. What stays `unsafe` is hardware access
(MMIO, DMA rings, cache maintenance) and the raw pointers of the C ABI's
callbacks, in short helpers with `SAFETY:` comments. Every module crate
denies `unsafe_op_in_unsafe_fn`.

### ABI versions

The comment above `ABI_VERSION` in `modules/abi/src/lib.rs` lists what
each version since v20 added. The notable ones:

| Version | What changed |
|---------|--------------|
| v60 | no `PersonalityOps::flags`: every riscv64 user task has the FPU on, so a personality need not ask |
| v59 | a PCI function's I/O BAR (`pci_io_map`), and a device of 32-bit pointers that DMA reaches (`DmaDevice::below`, `docs/pci.md`) |
| v56 | a board device's interrupt given back (`irq_disable`), the line masked once no handler is left on it; an interrupt domain's `mask` beside its `unmask` |
| v55 | a PCI function's interrupt given back (`pci_irq_disable`): its vector, handler slot and IOMMU remapping entry are free for the next device (`docs/pci.md`) |
| v54 | the PCI interrupt routes the firmware's `_PRT` gives (`pci_intx_route`), which x86_64 routes a function's INTx by; `phys_map`, physical memory for the acpi module's AML interpreter (`docs/platform.md` "AML") |
| v52 | `/dev/pci` gets a BAR's address as the CPUs see it (`pci_bar_phys`), whether a device's DMA sees the caches (`dma_coherent`), and non-cacheable mappings for one that does not (`MYOS_MMAP_NOCACHE`, `docs/pci.md`) |
| v51 | `irq_domain_register` / `irq_dispatch`, a module's secondary interrupt controller as an interrupt domain; `MmioDevice::phandle` |
| v50 | a file's `open` hook can name another file for the fd: opening a `/net` protocol's `clone` gives the new conversation's `data` (`docs/net.md`) |
| v45 | a filesystem's write hooks and `vfs_write` can report a full filesystem (`VFS_ENOSPC`, the program's `ENOSPC`) |
| v44 | the `poll` hooks get a change count, what an edge-triggered wait set member is reported against (`docs/events.md`) |
| v35 | a filesystem's `statfs` hook and `vfs_statfs`: how full it is |
| v34 | a filesystem's stat size is 64 bits: files and disks past 4 GiB |
| v33 | `irq_enable` (a board device's interrupt: a GIC SPI, a PLIC source, an ISA IRQ through the I/O APIC) and `console_input`; `keyboard_irq` / `cursor_blinks` in the console's ops: the keyboards' interrupt (`docs/pci.md`) |
| v32 | `current_uid`, the user the calling process runs for (`docs/security.md`) |
| v31 | `power_register`, a module's power-off or reboot method (`docs/power.md`) |
| v30 | a filesystem's `file_id` and `set_times_ino` hooks (the `*_ino` hooks are how an open file is reached); `fd_lockctl`, file locks for the Linux layer |
| v29 | a filesystem's `set_size` and `set_size_ino` hooks (`ftruncate`) |
| v28 | the hooks that keep a file unlinked while it is held: `unlink_keep`, `read_ino`, `write_ino`, `stat_ino`, `forget_ino` |
| v27 | `thread_place` |
| v26 | a filesystem's `unmount` hook (`umount`) |
| v25 | file times: a filesystem's `set_times` hook and `vfs_set_times` |
| v21 | `blk_unregister`; the service registry (`service_register` / `service_lookup`) that modules reach each other through; module threads (`thread_spawn`); `wake`; the USB bus types (`docs/usb.md`) |
| v20 | the `open` hook that makes a file exclusive (`/dev/console/kbd`) |
| v19 | `ioctl` taken out |

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
