# PCI

## Module / kernel split

| Component | Location | Why |
|-----------|----------|-----|
| PCI config access (`cfg_read32` / BAR map / `pci_find` / `pci_find_class`) | `kernel/src/pci.rs` + `arch/*/pci.rs` | The module ABI exposes it; every PCI driver is a module |
| virtio-blk, virtio-scsi, NVMe, SD cards | `modules/virtio_blk`, `modules/virtio_scsi`, `modules/nvme`, `modules/sdmmc` | Register `/dev/<name>` through `blk_register`; loaded before the filesystem modules. The virtio modules are the `virtio-drivers` crate over the module ABI (`modules/virtio`: its `Hal`, PCI and virtio-mmio probing) |
| USB: the core, the xHCI, EHCI, UHCI, OHCI and DWC2 hosts, hub and mass storage | `modules/usb`, `modules/xhci`, `modules/ehci`, `modules/uhci`, `modules/ohci`, `modules/dwc2`, `modules/usb_hub`, `modules/usb_storage` | The core publishes the USB bus as a service the class drivers look up and the controller drivers register with; sticks are `/dev/sdX/`, hot-pluggable (`docs/usb.md`) |
| Framebuffer text, keyboards, keymap | `modules/console` | Serial stays in the kernel; the module registers screen + keyboard ops (`console_register`) |
| Linux syscall layer | `modules/linux` | Registers a syscall *personality* (`personality_register`); the kernel keeps only which tasks have it (`kernel/src/personality.rs`) |
| Full PCI enumeration → `/proc/pci` | `modules/pci_enum` (`.ko`) | Discovery + on-demand rescan via write; talks only through `KernelApi` |
| Userspace SDIO drivers → `/dev/sdio` | `modules/sdmmc` | An SDIO card's functions' registers (CMD52, CMD53) and its interrupt for a driver that is a program: the Raspberry Pi's Wi-Fi chip (`docs/raspberry-pi.md` "SDIO") |
| Userspace PCI drivers → `/dev/pci` | `modules/pcidev` | A function's registers, DMA memory and interrupt for a driver that is a program (below); loaded with `insmod`, not at boot |
| The IOMMU | `kernel/src/iommu` | What a device's DMA reaches: all of memory for a kernel driver's, its own buffers for a program's (below) |
| Platform description (ACPI static tables, device tree) | `kernel/src/platform.rs`, `acpi.rs`, `dt.rs` | Device bases, interrupt routing and clocks come from it, before the heap exists |
| ACPI tables + AML → `/proc/acpi/*` | `modules/acpi` (`.ko`) | Optional; stubs when no RSDP. On x86_64 the `acpi` crate's AML interpreter: the PCI interrupt routes (`_PRT`) for the kernel and `_S5` ("AML" below) |
| Limine RSDP / MP requests | `kernel/src/limine_boot.rs` | Boot protocol |
| AP bring-up + `/proc/cpuinfo` | `kernel/src/smp.rs` | Must run before modules; owns CPU-local state + IPI helpers |
| Cross-CPU scheduler | `kernel/src/task/` | Per-CPU `CURRENT`, task `affinity`, per-CPU run queues |
| Proc exporters | `kernel/src/fs/procfs.rs` | Built-ins: `mounts`, `cpuinfo`, `platform`; dynamic via `proc_register` ABI |

ABI version: **21** (`blk_unregister`, `service_register` / `service_lookup`, `thread_spawn`, `wake` and the USB bus types, `docs/usb.md`; see `README.md` for 15–20; 14: `dt_mmio_find`: a module finds its memory-mapped devices in the device tree; 13: `personality_register`, `personality_exec`, `native_syscall` and the task / fd / VFS / signal / FPU / wait helpers a syscall personality needs: the Linux layer is a module; 12: `blk_register`, `pci_find_class`, `framebuffer_info`, `console_register`: block devices and the console are modules; 11: `pci_irq_enable`, `wake_any`, `wait_seq`, `block_until`, `monotonic_ns` for device interrupts and blocking waits; 10: `proc_set_writer` for `/proc/pci` rescan; earlier: `proc_register`, `acpi_rsdp`, `hhdm_offset`).

Boot modules come from the initramfs: `/lib/modules/<name>` for each name in `/lib/modules/boot.list` (`src/limine_image.rs` `BOOT_MODULES` plus the `OPTIONAL_MODULES` whose Cargo feature is on, in load order); `insmod <path>` (`SYS_INSMOD`) loads more at runtime and `/proc/modules` lists them.

## I/O BARs

A module reaches a function's I/O BAR (UHCI's registers) through
`KernelApi::pci_io_map` (ABI 59), which turns the function's I/O decoding
on and says where the registers are, and the abi crate's `PciIo`, which
reads and writes them: on x86_64 they are ports (`in`, `out`) the
firmware assigned; on aarch64 and riscv64 PCI I/O space is the host
bridge's I/O window, which the device tree's `ranges` name (space
`0x01`; QEMU `virt`: 64 KiB at 0x3eff0000 on aarch64, 0x03000000 on
riscv64), mapped at boot like the MMIO windows, and an I/O BAR the
firmware left at 0 is assigned in it from 0x1000 up
(`arch/*/pci.rs`, `io_assign`). `/proc/platform` lists the window
(`pci io`).

## DMA

A driver attaches each device it drives (`KernelApi::dma_attach`, a PCI
function or a memory-mapped device by its registers' physical address,
ABI 47) and gets a handle; `dma_alloc` gives that device zeroed,
contiguous pages it reaches and the address it uses for them, and
`dma_sync` hands memory over to it (`DMA_TO_DEVICE`: before it reads what
the CPU wrote, and before it writes, so that no dirty line is evicted over
its data later) and back (`DMA_FROM_DEVICE`: before the CPU reads what it
wrote). `kernel/src/dma.rs` keeps what it knows of each device:

- **What it reaches**: windows of CPU addresses `[cpu, cpu + size)`,
  each addressed from `bus` up, from the device tree's `dma-ranges` (a
  PCI function's from its host bridge's, then the buses' above; a
  memory-mapped device's from its buses'), one per entry; a device no
  `dma-ranges` limit reaches all of memory one-to-one. A Raspberry Pi 4's
  PCIe reaches the low 3 GiB at bus address 0. The pages come from the
  first window that has them (`mm::alloc_contiguous_pages` takes the
  range; the bump cursor only moves up, so a device that reaches low
  memory alone gets it while the cursor is there, at boot). A device of
  short pointers says so (`DmaDevice::below`, ABI 59: EHCI's, UHCI's and OHCI's
  32 bits) and its windows are cut below its limit.
- **Whether it sees the caches**: on aarch64 a device not known to be
  coherent is taken for not, as every aarch64 driver did before: one the
  tree does not mark `dma-coherent` (QEMU `virt` marks its devices, the
  Pi 4's PCIe is not marked), and one no tree describes (an ACPI-only
  board); `dma_sync` cleans (`dc cvac`) or cleans and invalidates
  (`dc civac`) the lines then, and `dma_alloc` writes the zeros back.
  riscv64 (no cache maintenance in riscv64imac) and x86_64 are coherent.
  Either way `dma_sync` is a barrier.
- `dma=noncoherent` on the kernel command line takes every device on
  aarch64 for non-coherent: the maintenance runs on QEMU (which does not
  model the caches, so it shows the paths work, not that they are
  needed).

`/proc/platform` has a `dma` line per device attached. The virtio modules'
library (`modules/virtio`) cannot tell `virtio_drivers` calls apart by
device: its pages are the last-attached device's, and a module's devices
are taken to share their DMA (QEMU's, coherent).

## Userspace PCI drivers (`/dev/pci`)

`modules/pcidev` hands a PCI function to a driver that is a program (the
Wi-Fi driver, issue #495): a directory per endpoint, `/dev/pci/BB:SS.F`.
The module ships in the image but is not in `boot.list`: root loads it,
`insmod /lib/modules/pcidev` (from `/data/etc/rc.local` on a machine that
wants it, `docs/install.md`), because on a board without an IOMMU a program
that drives a device reaches all of memory through it, as the kernel does
(issue #501); with one ("The IOMMU" below) the device is confined to the
program's buffers.
Its `info` says what it is (`id VVVV:DDDD`, `class CC:SS:PP`) and who
drives it (`driver nvme`); its `ctl` is the driver's, one at a time and
root only (the `dev` label too), and while it is held the others are
there.

A PCI function has one owner at a time (`kernel/src/pci.rs`, ABI 46): a
module that binds to it, its `pci_enable`, `pci_bar_map` or
`pci_irq_enable` from its `module_init` or `module_rescan`, or a claim
through `KernelApi::pci_claim` until `pci_release`, which is how
`/dev/pci` holds a function while a program drives it. Opening the `ctl`
of a function a kernel driver drives is refused: closing it would turn
the device off under its driver (`cat` of the boot disk's `ctl` did).

| File | What |
|------|------|
| `ctl` | held while open: closing it gives the interrupt back (the next driver routes it again); reads the memory BARs (`barN ADDR SIZE`: the address as the CPUs see it, `KernelApi::pci_bar_phys`, ABI 52, where a board's PCI window moves bus addresses, a Raspberry Pi 4's), the interrupt once routed (`irq msix\|msi\|intx`), the IOMMU domain the function is confined to (`iommu N`; no line on a board without an IOMMU), whether the device's DMA sees the caches (`dma coherent` or `dma noncoherent`, `KernelApi::dma_coherent`) and the DMA buffers (`dmaN BUS PAGES`, the address the device uses); takes `enable` (memory decoding and bus mastering on), `disable`, `irq` (route the interrupt), `dma PAGES` (a zeroed, physically contiguous buffer, the next `dmaN`) |
| `config` | the 256 bytes of configuration space, read and written at any offset |
| `barN` | a memory BAR, for `mmap(MAP_SHARED)`: uncached (x86 PCD\|PWT, aarch64 Device-nGnRE; riscv64 from the platform's attributes for the address), the module's `mmap` hook saying so (`MYOS_MMAP_UNCACHED`, ABI 46), and it stays so through `mprotect` |
| `irq` | a read waits for the interrupt and returns the count since the last one (8 bytes, little endian), `poll` says one is pending; an INTx line stays masked (the command register's interrupt disable) from its interrupt until a write to `irq` (a byte at least: the kernel passes an empty write to no file), once the driver quietened its device |
| `dmaN` | the buffer, for `mmap(MAP_SHARED)`: ordinary memory, cached, or non-cacheable for a device whose DMA does not see the caches (`dma noncoherent` in `ctl`, "DMA" above; the module's `mmap` hook says so, `MYOS_MMAP_NOCACHE`, ABI 52: x86 PCD\|PWT, aarch64 Normal non-cacheable, MAIR Attr3), so the program needs no cache maintenance; the module writes the zeros of a reused buffer back for such a device |

Closing `ctl` (the driver's exit too) turns bus mastering and memory
decoding off, so the device stops reaching memory and raising messages,
and keeps the DMA buffers for the next driver, zeroed: they are never
freed, since a mapping of one may outlive its fd. With an IOMMU the
device reaches those buffers and nothing else; without one the program
reaches all of memory through the device's DMA, as the kernel does
(issue #501). The test (`pcidev_smoke`,
tag `pcidev`, which loads the module first) drives QEMU's `edu` device, which every test boot gets
(`edu,dma_mask=...`: its own mask is 28 bits): a kernel driver's
function its `ctl` refused, the id, liveness and
factorial registers, an interrupt it raises, a DMA copy through a buffer
and back with the interrupt at its end, and, where `ctl` names an IOMMU
domain, a copy to the page after the buffer that the IOMMU refuses
(`/proc/iommu` counts it, the buffer stays untouched). It takes MSI (`edu` has no
MSI-X) on every board with an MSI controller (`pcidev_msi`: x86_64's
local APIC, the aarch64 GICs', the riscv64 AIA's IMSIC), and a shared
INTx line on the riscv64 PLIC board.
