# PCI

The kernel keeps PCI configuration access, DMA memory and interrupt
routing; every PCI driver is a kernel module or, through `/dev/pci`, a
program. This page says where the pieces live, how a driver reaches a
function's I/O BARs and DMA memory, and how a program drives a PCI
function from userspace. Interrupts have their own page
(`docs/interrupts.md`), and so do the IOMMU (`docs/iommu.md`) and the
platform description that names the host bridges (`docs/platform.md`).

## Kernel and modules

| Component | Location | Why |
|-----------|----------|-----|
| PCI config access, BAR mapping, `pci_find` | `kernel/src/pci.rs` + `arch/*/pci.rs` | The module ABI exposes it; every PCI driver is a module |
| Enumeration → `/proc/pci` | `modules/pci_enum` | Discovery, and a rescan on a write |
| Userspace PCI drivers → `/dev/pci` | `modules/pcidev` | A function's registers, DMA memory and interrupt for a program (below); loaded with `insmod`, not at boot |
| The IOMMU | `kernel/src/iommu` | What a device's DMA reaches (`docs/iommu.md`) |

The modules reach the kernel through `KernelApi` (`modules/abi`);
`docs/modules.md` has the kernel/module split and the ABI versions.

## I/O BARs

A module reaches a function's I/O BAR (UHCI's registers, for example)
with `KernelApi::pci_io_map`, which turns I/O decoding on and says where
the registers are, and the abi crate's `PciIo`, which reads and writes
them. On x86_64 they are I/O ports. On aarch64 and riscv64 they are in
the host bridge's I/O window, which the device tree's `ranges` name and
the kernel maps at boot; a BAR the firmware left at 0 is assigned there
(`arch/*/pci.rs`, `io_assign`). `/proc/platform` lists the window
(`pci io`).

## DMA

A driver attaches each device it drives with `KernelApi::dma_attach`
and gets a handle. A device is a PCI function, or a memory-mapped device
named by its registers' physical address. Then:

- `dma_alloc` gives that device zeroed, contiguous pages it reaches, and
  the address the device uses for them;
- `dma_sync` hands memory over to the device and back:
  - `DMA_TO_DEVICE` before the device reads what the CPU wrote, and also
    before it writes, so that no dirty line is evicted over its data
    later;
  - `DMA_FROM_DEVICE` before the CPU reads what the device wrote.

`kernel/src/dma.rs` keeps two things per device:

- **What it reaches**, from the device tree's `dma-ranges` of its host
  bridge and buses. A device no `dma-ranges` limits reaches all of memory
  one-to-one; a Raspberry Pi 4's PCIe reaches the low 3 GiB. A device
  with 32-bit pointers says so (`DmaDevice::below`: EHCI, UHCI, OHCI).
  The pages come from a contiguous allocator whose cursor only moves up,
  so a device that reaches low memory alone gets it at boot.
- **Whether it sees the caches.** On aarch64 a device the tree does not
  mark `dma-coherent`, or one no tree describes (an ACPI-only board), is
  taken for non-coherent: `dma_sync` then does the cache maintenance. The
  Pi 4's PCIe is such a device. riscv64 and x86_64 are coherent. Either
  way `dma_sync` is a barrier. `dma=noncoherent` on the kernel command
  line takes every aarch64 device for non-coherent; QEMU does not model
  the caches, so that shows the paths work, not that they are needed.

`/proc/platform` has a `dma` line per device attached
(`docs/platform.md`). The virtio library (`modules/virtio`) cannot tell
`virtio_drivers` calls apart by device, so a module's virtio devices
share the last-attached one's DMA (QEMU's, coherent).

## Userspace PCI drivers (`/dev/pci`)

`modules/pcidev` hands a PCI function to a driver that is a program, such
as the Wi-Fi driver (issue #495). Each endpoint is a directory,
`/dev/pci/BB:SS.F`.

The module ships in the image but is not loaded at boot: root loads it
with `insmod /lib/modules/pcidev`, from `/data/etc/rc.local` on a machine
that wants it (`docs/install.md`). On a board without an IOMMU, a program
that drives a device reaches all of memory through its DMA, as the
kernel does (issue #501). With an IOMMU the device is confined to the
program's buffers (`docs/iommu.md`).

A PCI function has one owner at a time (`kernel/src/pci.rs`): a module
that binds to it, or a claim through `KernelApi::pci_claim` until
`pci_release`, which is how `/dev/pci` holds it. Opening the `ctl` of a
function a kernel driver drives is refused, since closing it would turn
the device off under its driver.

### Files

A function's `info` says what it is (`id VVVV:DDDD`, `class CC:SS:PP`)
and who drives it (`driver nvme`). Its `ctl` is the driver's: one holder
at a time, root only (the `dev` label too). The other files are there
only while `ctl` is held.

| File | What |
|------|------|
| `ctl` | reads the memory BARs (`barN ADDR SIZE`, the address as the CPUs see it), the interrupt once routed (`irq msix\|msi\|intx`), the IOMMU domain (`iommu N`), `dma coherent` or `dma noncoherent`, and the DMA buffers (`dmaN BUS PAGES`, the address the device uses); takes `enable` (memory decoding and bus mastering), `disable`, `irq` (route the interrupt), `dma PAGES` (a new zeroed, contiguous buffer) |
| `config` | the 256 bytes of configuration space, read and written at any offset |
| `barN` | a memory BAR, for `mmap(MAP_SHARED)`, uncached |
| `irq` | a read waits for the interrupt and returns the count since the last one (8 bytes, little endian); `poll` says one is pending. An INTx line stays masked from its interrupt until a write to `irq` (a byte at least), once the driver quietened its device |
| `dmaN` | the buffer, for `mmap(MAP_SHARED)`: cached, or non-cacheable for a non-coherent device, so the program needs no cache maintenance |

Closing `ctl` (the driver's exit too) gives the interrupt back, turns bus
mastering and memory decoding off, and keeps the DMA buffers, zeroed,
for the next driver. They are never freed, since a mapping of one may
outlive its fd.

### The test

`pcidev_smoke` (`user/c`, tag `pcidev`, which loads the module first)
drives QEMU's `edu` device, which every test boot gets (as
`edu,dma_mask=...`, since its own mask is 28 bits). It checks the
refusal of a kernel driver's `ctl`, the device's registers, its
interrupt (MSI on every board with an MSI controller, a shared INTx line
on the riscv64 PLIC board), a DMA copy, and, with an IOMMU, that a DMA
past the buffer is refused and counted in `/proc/iommu`.
