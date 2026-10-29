# PCI

The kernel keeps PCI configuration access, DMA memory and interrupt
routing; every PCI driver is a kernel module or, through `/dev/pci`, a
program. This page lists where the pieces live, how a driver reaches a
function's I/O BARs and DMA memory, and how a program drives a PCI
function from userspace. Interrupts have their own page
(`docs/interrupts.md`), and so do the IOMMU (`docs/iommu.md`) and the
platform description that names the host bridges (`docs/platform.md`).

## Kernel and modules

The kernel keeps PCI configuration access, and every PCI driver is a
module. `docs/modules.md` "What stays in the kernel" has the whole split
between the kernel and the modules.

| Component | Location | Why |
|-----------|----------|-----|
| PCI config access (`cfg_read32` / BAR map / `pci_find` / `pci_find_class`) | `kernel/src/pci.rs` + `arch/*/pci.rs` | The module ABI exposes it; every PCI driver is a module |
| Full PCI enumeration → `/proc/pci` | `modules/pci_enum` (`.ko`) | Discovery + on-demand rescan via write; talks only through `KernelApi` |
| Userspace PCI drivers → `/dev/pci` | `modules/pcidev` | A function's registers, DMA memory and interrupt for a driver that is a program (below); loaded with `insmod`, not at boot |
| The IOMMU | `kernel/src/iommu` | What a device's DMA reaches: all of memory for a kernel driver's, its own buffers for a program's (`docs/iommu.md`) |

The modules reach the kernel through `KernelApi` (`modules/abi`). Its
version is `ABI_VERSION`, whose doc comment in `modules/abi/src/lib.rs`
lists what each version from 19 on changed (`docs/modules.md`).

## I/O BARs

A module reaches a function's I/O BAR (UHCI's registers, for example)
through two pieces:

- `KernelApi::pci_io_map` (ABI 59) turns the function's I/O decoding on
  and says where the registers are;
- the abi crate's `PciIo` reads and writes them.

Where the registers are depends on the arch:

- **x86_64**: they are I/O ports (`in`, `out`) that the firmware
  assigned.
- **aarch64 and riscv64**: PCI I/O space is the host bridge's I/O window,
  which the device tree's `ranges` name (space `0x01`; QEMU `virt`: 64 KiB
  at 0x3eff0000 on aarch64, 0x03000000 on riscv64). The kernel maps it at
  boot like the MMIO windows. An I/O BAR the firmware left at 0 is
  assigned in it from 0x1000 up (`arch/*/pci.rs`, `io_assign`).

`/proc/platform` lists the window (`pci io`).

## DMA

A driver attaches each device it drives with `KernelApi::dma_attach`
(ABI 47) and gets a handle. A device is a PCI function, or a
memory-mapped device named by its registers' physical address. Then:

- `dma_alloc` gives that device zeroed, contiguous pages it reaches, and
  the address the device uses for them;
- `dma_sync` hands memory over to the device (`DMA_TO_DEVICE`) and back
  (`DMA_FROM_DEVICE`):
  - `DMA_TO_DEVICE` before the device reads what the CPU wrote, and also
    before it writes, so that no dirty line is evicted over its data
    later;
  - `DMA_FROM_DEVICE` before the CPU reads what the device wrote.

`kernel/src/dma.rs` keeps what it knows of each device:

- **What it reaches.** These are windows of CPU addresses `[cpu, cpu +
  size)`, each addressed from `bus` up, one per entry of the device
  tree's `dma-ranges`. A PCI function's come from its host bridge's,
  then the buses' above; a memory-mapped device's from its buses'. A
  device that no `dma-ranges` limits reaches all of memory one-to-one. A
  Raspberry Pi 4's PCIe reaches the low 3 GiB at bus address 0.
  - The pages come from the first window that has them
    (`mm::alloc_contiguous_pages` takes the range). The bump cursor only
    moves up, so a device that reaches low memory alone gets it while the
    cursor is there, at boot.
  - A device with short pointers says so (`DmaDevice::below`, ABI 59:
    EHCI's, UHCI's and OHCI's 32 bits), and its windows are cut below its
    limit.
- **Whether it sees the caches.**
  - On aarch64 a device not known to be coherent is taken for
    non-coherent: one the tree does not mark `dma-coherent` (QEMU `virt`
    marks its devices; the Pi 4's PCIe is not marked), and one no tree
    describes (an ACPI-only board). `dma_sync` then cleans (`dc cvac`) or
    cleans and invalidates (`dc civac`) the lines, and `dma_alloc` writes
    the zeros back.
  - riscv64 (no cache maintenance in riscv64imac) and x86_64 are
    coherent.
  - Either way `dma_sync` is a barrier.
  - `dma=noncoherent` on the kernel command line takes every device on
    aarch64 for non-coherent. On QEMU the maintenance then runs; QEMU does
    not model the caches, so this shows the paths work, not that they are
    needed.

`/proc/platform` has a `dma` line per device attached
(`docs/platform.md`). The virtio modules' library (`modules/virtio`)
cannot tell `virtio_drivers` calls apart by device: its pages are the
last-attached device's, and a module's devices are taken to share their
DMA (QEMU's, coherent).

## Userspace PCI drivers (`/dev/pci`)

`modules/pcidev` hands a PCI function to a driver that is a program, such
as the Wi-Fi driver (issue #495). Each endpoint is a directory,
`/dev/pci/BB:SS.F`.

The module ships in the image but is not in `boot.list`: root loads it
with `insmod /lib/modules/pcidev`, from `/data/etc/rc.local` on a machine
that wants it (`docs/install.md`). The reason is that on a board without
an IOMMU, a program that drives a device reaches all of memory through it,
as the kernel does (issue #501). With an IOMMU the device is confined to
the program's buffers (`docs/iommu.md`).

A function's `info` says what it is (`id VVVV:DDDD`, `class CC:SS:PP`)
and who drives it (`driver nvme`). Its `ctl` is the driver's: one holder
at a time, root only (the `dev` label too). The function's other files
(below) are there only while `ctl` is held.

### Ownership

A PCI function has one owner at a time (`kernel/src/pci.rs`, ABI 46):

- a module that binds to it, by its `pci_enable`, `pci_bar_map` or
  `pci_irq_enable` from its `module_init` or `module_rescan`;
- or a claim through `KernelApi::pci_claim`, until `pci_release`. This is
  how `/dev/pci` holds a function while a program drives it.

Opening the `ctl` of a function that a kernel driver drives is refused:
closing it would turn the device off under its driver.

### Files

| File | What |
|------|------|
| `ctl` | held while open: closing it gives the interrupt back (the next driver routes it again); reads the memory BARs (`barN ADDR SIZE`: the address as the CPUs see it, `KernelApi::pci_bar_phys`, ABI 52, where a board's PCI window moves bus addresses, a Raspberry Pi 4's), the interrupt once routed (`irq msix\|msi\|intx`), the IOMMU domain the function is confined to (`iommu N`; no line on a board without an IOMMU), whether the device's DMA sees the caches (`dma coherent` or `dma noncoherent`, `KernelApi::dma_coherent`) and the DMA buffers (`dmaN BUS PAGES`, the address the device uses); takes `enable` (memory decoding and bus mastering on), `disable`, `irq` (route the interrupt), `dma PAGES` (a zeroed, physically contiguous buffer, the next `dmaN`) |
| `config` | the 256 bytes of configuration space, read and written at any offset |
| `barN` | a memory BAR, for `mmap(MAP_SHARED)`: uncached (x86 PCD\|PWT, aarch64 Device-nGnRE; riscv64 from the platform's attributes for the address), the module's `mmap` hook saying so (`MYOS_MMAP_UNCACHED`, ABI 46), and it stays so through `mprotect` |
| `irq` | a read waits for the interrupt and returns the count since the last one (8 bytes, little endian), `poll` says one is pending; an INTx line stays masked (the command register's interrupt disable) from its interrupt until a write to `irq` (a byte at least: the kernel passes an empty write to no file), once the driver quietened its device |
| `dmaN` | the buffer, for `mmap(MAP_SHARED)`: ordinary memory, cached, or non-cacheable for a device whose DMA does not see the caches (`dma noncoherent` in `ctl`, "DMA" above; the module's `mmap` hook says so, `MYOS_MMAP_NOCACHE`, ABI 52: x86 PCD\|PWT, aarch64 Normal non-cacheable, MAIR Attr3), so the program needs no cache maintenance; the module writes the zeros of a reused buffer back for such a device |

Closing `ctl` (the driver's exit too):

- turns bus mastering and memory decoding off, so the device stops
  reaching memory and raising messages;
- keeps the DMA buffers for the next driver, zeroed. They are never
  freed, since a mapping of one may outlive its fd.

With an IOMMU the device reaches those buffers and nothing else. Without
one the program reaches all of memory through the device's DMA, as the
kernel does (issue #501).

### The test

The test (`pcidev_smoke`, tag `pcidev`, which loads the module first)
drives QEMU's `edu` device. Every test boot gets one, as `edu,dma_mask=...`,
since its own mask is 28 bits. The test checks:

- that the `ctl` of a kernel driver's function is refused;
- the id, liveness and factorial registers;
- an interrupt the device raises;
- a DMA copy through a buffer and back, with the interrupt at its end;
- where `ctl` names an IOMMU domain, a copy to the page after the buffer,
  which the IOMMU refuses: `/proc/iommu` counts it and the buffer stays
  untouched.

It takes MSI (`edu` has no MSI-X) on every board with an MSI controller
(`pcidev_msi`: x86_64's local APIC, the aarch64 GICs', the riscv64 AIA's
IMSIC), and a shared INTx line on the riscv64 PLIC board.

## History

- Opening a kernel-driven function's `ctl` is refused because a `cat` of
  the boot disk's `ctl` once turned the disk off under its driver.
- Early module ABI versions, before the list in `ABI_VERSION`'s doc
  comment begins: 10 added `proc_set_writer` (the `/proc/pci` rescan); 11
  `pci_irq_enable`, `wake_any`, `wait_seq`, `block_until` and
  `monotonic_ns` (device interrupts and blocking waits); 12
  `blk_register`, `pci_find_class`, `framebuffer_info` and
  `console_register` (block devices and the console became modules); 13
  `personality_register`, `personality_exec`, `native_syscall` and the
  task / fd / VFS / signal / FPU / wait helpers a syscall personality
  needs (the Linux layer became a module); 14 `dt_mmio_find` (a module
  finds its memory-mapped devices in the device tree). Earlier versions
  had `proc_register`, `acpi_rsdp` and `hhdm_offset`.
