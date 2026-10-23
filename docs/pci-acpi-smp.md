# PCI discovery, ACPI (custom AML), and SMP scheduler

## Module / kernel split

| Component | Location | Why |
|-----------|----------|-----|
| PCI config access (`cfg_read32` / BAR map / `pci_find` / `pci_find_class`) | `kernel/src/pci.rs` + `arch/*/pci.rs` | The module ABI exposes it; every PCI driver is a module |
| virtio-blk, virtio-scsi, NVMe | `modules/virtio_blk`, `modules/virtio_scsi`, `modules/nvme` | Register `/dev/<name>` through `blk_register`; loaded before the filesystem modules. The virtio modules are the `virtio-drivers` crate over the module ABI (`modules/virtio`: its `Hal`, PCI and virtio-mmio probing) |
| USB: xHCI host, hub and mass storage | `modules/xhci`, `modules/usb_hub`, `modules/usb_storage` | The host publishes the USB bus as a service the class drivers look up; sticks are `/dev/sdX/`, hot-pluggable (`docs/usb.md`) |
| Framebuffer text, keyboards, keymap | `modules/console` | Serial stays in the kernel; the module registers screen + keyboard ops (`console_register`) |
| Linux syscall layer | `modules/linux` | Registers a syscall *personality* (`personality_register`); the kernel keeps only which tasks have it (`kernel/src/personality.rs`) |
| Full PCI enumeration → `/proc/pci` | `modules/pci_enum` (`.ko`) | Discovery + on-demand rescan via write; talks only through `KernelApi` |
| Userspace PCI drivers → `/dev/pci` | `modules/pcidev` | A function's registers, DMA memory and interrupt for a driver that is a program (below); loaded with `insmod`, not at boot |
| The IOMMU | `kernel/src/iommu` | What a device's DMA reaches: all of memory for a kernel driver's, its own buffers for a program's (below) |
| Platform description (ACPI static tables, device tree) | `kernel/src/platform.rs`, `acpi.rs`, `dt.rs` | Device bases, interrupt routing and clocks come from it, before the heap exists |
| ACPI tables + AML → `/proc/acpi/*` | `modules/acpi` (`.ko`) | Optional; stubs when no RSDP |
| Limine RSDP / MP requests | `kernel/src/limine_boot.rs` | Boot protocol |
| AP bring-up + `/proc/cpuinfo` | `kernel/src/smp.rs` | Must run before modules; owns CPU-local state + IPI helpers |
| Cross-CPU scheduler | `kernel/src/task/` | Per-CPU `CURRENT`, task `affinity`, per-CPU run queues |
| Proc exporters | `kernel/src/fs/procfs.rs` | Built-ins: `mounts`, `cpuinfo`, `platform`; dynamic via `proc_register` ABI |

ABI version: **21** (`blk_unregister`, `service_register` / `service_lookup`, `thread_spawn`, `wake` and the USB bus types, `docs/usb.md`; see `README.md` for 15–20; 14: `dt_mmio_find`: a module finds its memory-mapped devices in the device tree; 13: `personality_register`, `personality_exec`, `native_syscall` and the task / fd / VFS / signal / FPU / wait helpers a syscall personality needs: the Linux layer is a module; 12: `blk_register`, `pci_find_class`, `framebuffer_info`, `console_register`: block devices and the console are modules; 11: `pci_irq_enable`, `wake_any`, `wait_seq`, `block_until`, `monotonic_ns` for device interrupts and blocking waits; 10: `proc_set_writer` for `/proc/pci` rescan; earlier: `proc_register`, `acpi_rsdp`, `hhdm_offset`).

Boot modules come from the initramfs: `/lib/modules/<name>` for each name in `/lib/modules/boot.list` (`src/limine_image.rs` `BOOT_MODULES` plus the `OPTIONAL_MODULES` whose Cargo feature is on, in load order); `insmod <path>` (`SYS_INSMOD`) loads more at runtime and `/proc/modules` lists them.

## Interrupt controllers

Every arch drives its root interrupt controller through one interface,
`irqchip::IrqChip` (`kernel/src/irqchip.rs`): `init_cpu` (the BSP's side,
then each AP's), `route` and `unmask` (a device's interrupt to the BSP),
`claim` and `complete` (what the interrupt entry takes and ends), and
`send_ipi` (TLB shootdown, reschedule; to one CPU or to every other).
Each arch lists the drivers it has in `arch::IRQCHIPS`:

| Arch | Driver | Device tree `compatible` | ACPI | File |
|------|--------|--------------------------|------|------|
| x86_64 | `apic`: the local APICs and the I/O APIC | — | MADT (also without it) | `arch/x86/apic.rs` |
| aarch64 | `gicv2` | `arm,cortex-a15-gic`, `arm,gic-400`, `arm,cortex-a9-gic` | MADT GICD + GICC | `arch/aarch64/gic.rs` |
| aarch64 | `gicv3` | `arm,gic-v3` | MADT GICD + GICR | `arch/aarch64/gic.rs` |
| aarch64 | `bcm2836`: the Raspberry Pi 3's local and peripheral controllers | `brcm,bcm2836-l1-intc` + `brcm,bcm2836-armctrl-ic` | — | `arch/aarch64/bcm2836.rs` |
| riscv64 | `plic` (with the SBI's IPIs) | `sifive,plic-1.0.0`, `riscv,plic0` | — | `arch/riscv64/plic.rs` |

A driver (`irqchip::Driver`) is what is known before the controller runs:
its name, the device tree nodes it is made of (each with its
`compatible` strings and the number of `reg` entries it takes), `xlate`
(an interrupt specifier of one of its nodes to an interrupt number) and
`start` (its registers mapped from the description, the running
controller returned). The platform description names the driver
(`intc gicv2 ...` in `/proc/platform`, below): `dt::fill` takes the first
of the arch's drivers whose nodes the tree has, the MADT names a GIC or
the APICs, and `irqchip::start` starts it from `arch::apply_platform`
(x86_64 starts `apic` without a description too). A device tree
interrupt specifier is decoded by the driver its `interrupt-parent` (or
the host bridge's `interrupt-map` entry) names, whichever controller the
board's root is (`irqchip::xlate`); one whose parent no driver is made of
is not delivered.

Adding a controller is a file implementing `IrqChip` with its `Driver`,
and the driver in its arch's `IRQCHIPS`. aarch64's controllers number
their interrupts as the GIC does (SGI 0 and 1 the IPIs, PPI 27 and 30
the timers, 32 and up the devices), which the interrupt entry reads.

### Interrupt domains

A secondary controller, whose lines reach the CPU through one interrupt
of the root's (a GPIO block's pins, a PCIe bridge's MSI), is a module:
an interrupt domain (`KernelApi::irq_domain_register`, ABI 51). The
module drives the controller; the kernel gives the domain a range of
interrupt numbers from 4096 up (`irqchip::DOMAIN_FIRST`; at most 8
domains, 4096 lines between them), line `n` being `first + n`:

- a driver of a device on a line enables its interrupt as any other
  (`irq_enable`); the kernel registers the handler, then calls the
  domain's `unmask` with the line;
- the module enables the controller's own interrupt, and its handler
  passes each line pending to `KernelApi::irq_dispatch` (an edge is
  cleared before, a level after its handlers);
- a device tree specifier whose `interrupt-parent` is the domain's node
  (`MmioDevice::phandle`) decodes to its line, the first cell
  (`irqchip::xlate`).

A domain stays while the kernel runs: it is a registration, so the
module stays loaded. The first is `modules/pl061`, the ARM PL061 GPIO
controller's 8 lines (QEMU `virt` without ACPI: with ACPI tables its
place is the ACPI GED's). No driver uses a line yet, so it ships for
`insmod` and is not loaded at boot; its probe checks the path once: it
sets an idle input to interrupt while low and enables that line through
the kernel, and its handler turns it off (`/proc/interrupts`: `pl061`,
the controller's, and `pl061 check`, the line's; the `irq_domain` test).

## Device interrupts

`kernel/src/irq.rs` keeps one handler per interrupt number and `pci_irq_setup`
picks the delivery mechanism per arch, so a module only asks for "this PCI
function's interrupt" (`KernelApi::pci_irq_enable`) and gets told whether to
use an MSI-X table entry or the INTx line:

| Arch | Mechanism | Numbering |
|------|-----------|-----------|
| x86_64 | MSI-X entry 0 → LAPIC vector on the BSP (`DEVICE_VECTOR_BASE` 48 + n, 16 vectors shared with the I/O APIC pins below; no PIRQ routing; a BSP with an x2APIC id above 255 would need interrupt remapping, so it gets no MSI); a function without MSI-X gets its one MSI message the same way (`MYOS_IRQ_MSI`) | vector |
| aarch64 | INTx → GIC SPI, level, priority 0x80, delivered to the BSP (GICv2: `ITARGETSR` CPU 0; GICv3: `IROUTER` to its affinity); the SPI comes from the device tree's PCIe `interrupt-map` (QEMU `virt`: SPI 3 + (slot + pin − 1) mod 4) | INTID (35..38) |
| riscv64 | INTx → PLIC source for the boot hart's S-mode context, from the same `interrupt-map` (`virt`: 32 + (slot + pin − 1) mod 4), `sie.SEIE` | PLIC source |

A board device's own interrupt goes through `irq::enable` (modules:
`KernelApi::irq_enable`, ABI 33), which registers the handler before the
interrupt controller unmasks the line. On aarch64 and riscv64 the number
is the one the device tree (decoded by its controller's driver) or the
SPCR gives (a GIC SPI, a PLIC source; `MmioDevice::irq` for a module). On x86_64 it is a legacy ISA IRQ: `arch/x86/ioapic.rs` drives the
MADT's first I/O APIC, takes the GSI and polarity/trigger from the
interrupt source overrides, and points the pin at a fresh device vector
on the BSP (the 8259s stay masked). The console's input uses it: the UART's
receive interrupt (COM1's IRQ 4, the SPCR's or device tree's on the
others), the PS/2 keyboard's IRQ 1, the PS/2 mouse's IRQ 12 and the
virtio-input devices' (the keyboard's and the pointers'). A PC
without an MADT I/O APIC routes nothing: the UART is then drained by CPU
0's tick and the keyboard polled, as before.

## The platform description

Nothing about the board is hard-coded: `kernel/src/platform.rs` describes
it once at boot, before the heap and the first console output, and the
arch code takes its device bases from that description and from nowhere
else (`arch::apply_platform`). Two sources fill it, in this order:

1. **The ACPI static tables** (`kernel/src/acpi.rs`), when the firmware
   hands Limine an RSDP and the arch prefers them (`arch::PREFER_ACPI`:
   x86_64 and aarch64; not riscv64, whose ACPI tables are younger than
   the firmware that runs here). Only tables that name the platform are
   read, and no AML: the MADT (CPUs; the local APIC, or the GICD, GICC,
   GICR and ITS entries), the MCFG (PCIe ECAM and bus range), the SPCR
   (the console UART and its interrupt), the GTDT (the arm timer
   interrupts) and the FADT's arm boot flags (PSCI). Allocation-free,
   read in place through the HHDM. `modules/acpi` keeps `/proc/acpi` and
   the ACPI power methods (`_S5` power-off, the reset register:
   `docs/power.md`).
2. **The device tree** (`kernel/src/dt.rs`, the `fdt` crate, MPL-2.0),
   when Limine hands one over. It fills what nothing described yet and is
   compared against what the tables did: a component both describe
   differently is flagged (`[WARN] platform: acpi and dt differ on ...`
   at boot, `differs` in `/proc/platform`) rather than silently resolved
   in favour of either. Nodes whose `status` is not `okay` are skipped,
   and a `reg` is translated through the `ranges` of the buses above it
   (the Raspberry Pi 4's peripherals sit at bus address `0x7e00_0000`
   behind a `simple-bus`, `docs/raspberry-pi.md`).

| Component | ACPI | Device tree (`compatible`) |
|------|------|------|
| CPUs | MADT enabled LAPIC / x2APIC / GICC entries | `/cpus` children |
| Interrupt controller | MADT: LAPIC address; GICD (with the GIC version) + GICC → GICv2, GICD + GICR → GICv3 | the nodes of the arch's drivers ("Interrupt controllers" above: the GICs, the Raspberry Pi 3's two, `docs/raspberry-pi.md`, the PLIC) |
| Console UART | SPCR: interface type (16550 or PL011 / SBSA), address, GSIV | the one `/chosen` `stdout-path` names (through `/aliases`), else the first `arm,pl011`, else the first `ns16550a` / `ns16550` / `snps,dw-apb-uart` / `brcm,bcm2835-aux-uart` (the Pi's mini UART); + `interrupts` |
| RTC | — | `arm,pl031`, `google,goldfish-rtc` (optional: without one the wall clock counts from the epoch at boot until `settimeofday` sets it) |
| Timers | GTDT: EL1 physical and virtual timer INTIDs | `/cpus` `timebase-frequency` (riscv64 `time` CSR rate) |
| PCIe host bridge | MCFG: ECAM base, bus range | `pci-host-ecam-generic`: `reg` (the ECAM window), `bus-range`; `brcm,bcm2711-pcie` (aarch64, the Raspberry Pi 4's): `reg` (its registers, below) |
| PCIe MMIO windows, INTx routing | — (in AML) | `ranges` (the 32-bit entry on aarch64, the 64-bit one on riscv64; CPU and bus address, which may differ), `interrupt-map` / `interrupt-map-mask` |
| DMA (`kernel/src/dma.rs`, below) | — (devices coherent, all of memory) | the host bridge's `dma-ranges`, a node's buses' `dma-ranges`; `dma-coherent` / `dma-noncoherent` on the node or a bus above |
| The IOMMU (`kernel/src/iommu`, below) | aarch64: the IORT's SMMUv3 node and the root complex's ID mapping | `arm,smmu-v3` (`reg`, `interrupts` by `interrupt-names`, `dma-coherent`), the PCIe host's `iommu-map`; x86_64's VT-d units are the DMAR table's, read by `iommu/vtd.rs` itself |
| virtio-mmio transports | — | `virtio,mmio`, for modules through `KernelApi::dt_mmio_find`, in ascending address order |
| PSCI (aarch64 power-off and reset, `docs/power.md`) | FADT arm boot flags: PSCI compliant, HVC or SMC | `arm,psci-1.0` / `arm,psci-0.2` / `arm,psci`: `method` |

What each arch requires of the description: aarch64 an interrupt
controller it has a driver for (a GICv2: the distributor and the CPU
interface; a GICv3: the distributor and the redistributor region, below;
a Raspberry Pi 3's two controllers), a PL011 or a 16550 (with its register stride
and access width: the tree's `reg-shift` / `reg-io-width`, the SPCR's
access size), and a PCIe host bridge, when it has one, with a 32-bit MMIO
window (one the kernel cannot drive leaves the board without PCI); riscv64 a PLIC, a
16550, the timebase and a host bridge with a 64-bit window; x86_64
takes the MCFG's ECAM window for PCI configuration space when the
description has one (QEMU `q35`, every PCIe PC; `arch/x86/pci.rs` maps a
bus's megabyte of it on first use, once the heap's frames exist) and
needs nothing else (the LAPIC base comes from its MSR, the console from
COM1, configuration space from port 0xCF8 without an MCFG), so a PC
without ACPI tables boots too (QEMU's `pc` machine publishes neither an
SPCR nor an MCFG: its description is the CPUs and the LAPIC).
The x86 boots (`cargo run`, the tests, CI) are the PCIe PC, `q35`, with
Intel's IOMMU (`-device intel-iommu`, "The IOMMU" below);
`MYOS_X86_MACHINE=pc cargo run -- test-mini` boots QEMU's default machine,
without ACPI's description of PCI and without an IOMMU. A missing source or component stops the boot with `fatal: platform:
...` on the UART at QEMU `virt`'s address (the one assumption left, so
the message has somewhere to go).

`/proc/platform` shows the description, one line per component, each
ending in the source it came from:

```
source acpi+dt
model linux,dummy-virt (dt)
oem BOCHS (acpi)
cpus 4 (acpi)
intc gicv2 0x8000000 0x8010000 (acpi)
uart pl011 0x9000000 irq 33 (acpi)
rtc pl031 0x9010000 (dt)
timer irq 30 27 (acpi)
pci ecam 0x3f000000 0x1000000 bus 0-15 (acpi)
pci mmio32 0x10000000 0x2eff0000 (dt)
dma mmio 0xa003e00 coherent
dma pci 00:02.0 coherent
```

A window whose CPU and bus addresses differ says both (`pci mmio32
0x600000000 0x40000000 bus 0xc0000000`, a Raspberry Pi 4's), and so does a
device whose DMA reaches part of memory only (`dma pci 01:00.0 0x0
0xc0000000 noncoherent`, its USB controller: the low 3 GiB, not coherent).

The kernel command line (`cmdline:` in `limine.conf`) forces one source
when a board's firmware gets the other wrong: `platform=acpi` ignores the
tree, `platform=dt` the tables (on riscv64 it also makes the kernel read
tables it would otherwise skip: `platform=acpi` there is the opt-in).

**GICv3** (`arch/aarch64/interrupts.rs`): the distributor runs with
affinity routing (every SPI in group 1, routed to the BSP's MPIDR
affinity by `GICD_IROUTER` when a driver enables it); each CPU finds its
redistributor by walking the region's frames (128 KiB each, 256 KiB with
the GICv4 VLPI frames) for the `GICR_TYPER` naming its affinity, wakes
it, enables its SGIs and timer PPIs there, and drives the CPU interface
through the `ICC_*` system registers (`ICC_SRE_EL2` as well when the
kernel runs at EL2; `ICC_IAR1`/`ICC_EOIR1` in the handler, `ICC_SGI1R`
for the IPIs with the target's affinity). The boot log says which GIC
(`[INFO] gicv3`). QEMU `virt` gives a GICv3 with `gic-version=3`:
`MYOS_AARCH64_GIC=3 cargo run -- aarch64 test-mini` (the launcher dumps
the matching tree); CI boots both versions (`aarch64`, `aarch64-gicv3`).
The GICv3 boots also have no ACPI tables (`acpi=off`): the firmware
hands the device tree alone (`source dt`), so CI boots an aarch64 board
described by the tree only, and the board has the PL061 (above).
The ITS (message-signalled interrupts) is not driven: PCI devices use
INTx on aarch64.

The EDK2 firmware boots (AAVMF, RISC-V EDK2) hand Limine no tree, so the
host tool dumps QEMU's (`-machine ...,dumpdtb`, same machine options,
firmware and `-smp` as the boot: QEMU builds a board with ACPI tables
differently) into the ESP as `boot/virt-aarch64.dtb` /
`boot/virt.dtb` and `limine.conf` passes it with `global_dtb`; AAVMF also
publishes ACPI tables, so the aarch64 boots exercise the agreement check
(`source acpi+dt`). Interrupt specifiers of the tree are decoded by the
driver of their interrupt parent (`irqchip::xlate`: GIC `<type number
flags>` → INTID, the Pi 3's `<bank number>`, PLIC `<source>`).

Handlers run in interrupt context with interrupts masked on CPU 0. On INTx
the handler must read the device's ISR register so the level line deasserts
before the GIC/PLIC EOI. An INTx line is shared by every slot the swizzle
puts on it (QEMU `virt` has four for all of them): each driver of a device
on it registers its handler, and every one runs at each interrupt of the
line, its device saying whether it was the one. `/proc/interrupts` lists
counts per registered handler plus spurious interrupts.

virtio-net uses this for its RX queue: `AVAIL_F_NO_INTERRUPT` is cleared on
the RX ring only, the handler acks the ISR and calls `wake_any`, which ends
the `poll` `netd` sleeps in on `/dev/net0/data` and its netfs channel (the
kernel's poll reads the wait sequence before it asks the module's readiness
hook, so a frame that lands between the check and the sleep is not missed).
The NIC's `ctl` says `irq on` when that works; on `irq off` (a poll-mode
device) `netd` keeps its 2–10 ms timed polling. Reads of `/dev/netN/data`
stay non-blocking.

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
  memory alone gets it while the cursor is there, at boot).
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
| `ctl` | reads the memory BARs (`barN ADDR SIZE`: the address as the CPUs see it, `KernelApi::pci_bar_phys`, ABI 52, where a board's PCI window moves bus addresses, a Raspberry Pi 4's), the interrupt once routed (`irq msix\|msi\|intx`), the IOMMU domain the function is confined to (`iommu N`; no line on a board without an IOMMU), whether the device's DMA sees the caches (`dma coherent` or `dma noncoherent`, `KernelApi::dma_coherent`) and the DMA buffers (`dmaN BUS PAGES`, the address the device uses); takes `enable` (memory decoding and bus mastering on), `disable`, `irq` (route the interrupt), `dma PAGES` (a zeroed, physically contiguous buffer, the next `dmaN`) |
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
(`/proc/iommu` counts it, the buffer stays untouched). It takes MSI on x86 (`edu` has no
MSI-X) and a shared INTx line on aarch64 and riscv64.

## The IOMMU

A PCI device reads and writes memory by the addresses its driver gives
it, and a program driving one through `/dev/pci` gives them itself: a bug
in it, or its device's own firmware, could reach kernel memory or another
process's pages. The IOMMU (`kernel/src/iommu`, issue #501) is the
platform's unit between the devices and memory, and the kernel programs
it so that:

- a device a kernel module drives stays in passthrough: the *identity
  domain* maps all of memory one-to-one, with the largest pages the unit
  has, so the drivers need nothing from it;
- a function a program holds through `/dev/pci` gets a domain of its own
  (`KernelApi::iommu_attach`, ABI 49, at the `ctl` open), holding only the
  DMA buffers the program asked for (`iommu_map`, at each `dma PAGES`,
  and the ones kept from the last driver), each at its physical address,
  so the numbers `ctl` shows stay the device's; closing `ctl` puts the
  function back in passthrough (`iommu_detach`);
- a DMA anywhere else is refused by the unit and recorded: the unit's
  fault event is an interrupt (an MSI to a device vector), its handler
  reads the fault recording registers. `/proc/iommu` lists the units,
  the domains with what they map, `faults N` and the last one
  (`last BB:SS.F read|write ADDRESS reason R`).

MSIs are DMA writes too; they still pass as they are (interrupt remapping
stays listed in issue #501).

| Arch | Unit | Where it is described |
|---|---|---|
| x86_64 | Intel VT-d (`iommu/vtd.rs`): a root table per unit (one entry per bus), a context table per bus (one entry per device and function), second-level page tables of the CPU's shape (3 levels for 39-bit addresses, 4 for 48, 2 MiB and 1 GiB superpages where `CAP.SLLPS` has them), invalidation through the registers (whole: the context cache and the IOTLB), the tables' cache lines flushed for a unit whose walks do not snoop (`ECAP.C` clear) | the ACPI DMAR table: each DRHD's registers and device scopes (endpoints and bridges), the `INCLUDE_PCI_ALL` one for the rest |
| aarch64 | Arm SMMUv3 (`iommu/smmuv3.rs`): a linear stream table, one entry per requester id, *bypass* for every function (no table walked) and a *stage-2* entry (VMSAv8, 4 KiB granule, a VMID per domain) for one a program holds; the command queue for invalidations and syncs, the event queue (its interrupt, which the SMMU pulses, taken edge-triggered) for the refused DMAs; the tables and queues written back for a unit that does not snoop (`IDR0.COHACC` clear) | the IORT's SMMUv3 node (registers, the event and global error GSIVs) and the root complex node's ID mapping to it, or the tree's `arm,smmu-v3` node and the PCIe host's `iommu-map`: a `platform::Iommu` component, `/proc/platform`'s `iommu smmuv3 ...` line |
| riscv64 | the RISC-V IOMMU (`iommu/riscv.rs`, specification 1.0): a three-level device directory, one 64-byte context per device id (32 bytes for a unit without MSI translation), *bare* for every function of a populated bus (no table walked) and a *second-stage* context (a G-stage table in the CPU's Sv39x4 or Sv48x4 format, a GSCID per domain) for one a program holds; the command queue for invalidations and fences, the fault queue for the refused DMAs, read when `/proc/iommu` is (the PCI unit signals it by MSI only, which the riscv64 kernel does not take) | a PCI function of class 08.06 (system peripheral, IOMMU), its registers in BAR 0, claimed by the kernel so `/dev/pci` cannot hand it out; nothing from the tree |

QEMU's `q35` carries the unit (`-device intel-iommu`, 39-bit, 3-level,
both superpage sizes, snooping walks), every x86 boot of the launcher and
of CI, QEMU's `virt` an SMMUv3 (`iommu=smmuv3`: 44-bit output,
snooping, SPIs 74 to 77 for its queues) on every aarch64 boot, where
`[ OK ] iommu: smmuv3 at 0x9050000, 4096 streams, 44-bit stage 2` is the
boot line, and the riscv64 `virt` the RISC-V IOMMU (`-device
riscv-iommu-pci`, first on the bus as `00:01.0`, Sv39x4 to Sv57x4, MSI
translation) on every boot whose QEMU has it: the device is QEMU 9.2's and later
(CI's image has it, Ubuntu 24.04's 8.2 has not); without it the launcher
says so once and boots without, `/proc/iommu` saying `none`.
Its boot line: `[ OK ] iommu: riscv unit at 00:01.0, 256 device ids,
Sv39x4 second stage`; `-device intel-iommu` is not for the `pc` machine
(`MYOS_X86_MACHINE=pc`), whose boot has no IOMMU. The UEFI boots run
OVMF from edk2-stable202505 (the launcher's `OVMF` constant): the
edk2-stable202605 build raises an invalid-opcode exception inside the
firmware on a `q35` with the IOMMU and an xHCI controller, before any boot
loader runs. On the `[ OK ]` line at
boot: `iommu: vt-d, 1 unit(s), 3-level tables, 1 GiB superpages, memory
to 0x...`; a board without a DMAR table prints nothing and `/proc/iommu`
says `none`.

## AML opcode set (custom interpreter — not ACPICA)

Used to resolve `Name (_S5_, Package …)` in the DSDT for `/proc/acpi/s5`
and the ACPI power-off (`docs/power.md`). The name is found by its bytes:
the segment `_S5_` right after a NameOp (`0x08`, the root prefix `\` may
come between) and followed by a PackageOp. The opcodes are not walked:
staying in step would take every one of them (a `0x08` in a buffer is no
NameOp).

| Opcode | Encoding | Role |
|--------|----------|------|
| Zero / One / Ones | `0x00` / `0x01` / `0xFF` | Integer constants |
| BytePrefix | `0x0A` | `u8` |
| WordPrefix | `0x0B` | `u16` LE |
| DWordPrefix | `0x0C` | `u32` LE |
| QWordPrefix | `0x0E` | `u64` LE |
| NameOp | `0x08` | before `_S5_` |
| PackageOp | `0x12` | read `slp_typa` / `slp_typb` |
| ReturnOp | `0xA4` | unwrap integer |

PkgLength encoding is implemented. Nothing is executed.

## Blocking waits, idle CPUs and wakeups

Tasks have a `Blocked` state. Every kernel wait — `wait`/`waitpid`, console
`read`, pipe/FIFO/pty read/write/open, `sigsuspend`/`sigwait`, `nanosleep`,
`poll`, the Linux layer's `nanosleep`/`ppoll` — follows one pattern:

```
loop {
    let seq = task::wait_seq();      // before checking the condition
    if condition_met { break }
    if signal::interrupt_wait() { return EINTR }
    task::block_until(key, seq, deadline_ns);   // deadline 0 = none
}
```

Producers change state and call `task::wake(key)` (`key_pipe(id)`,
`key_pty(id)`, `key_child(parent)`, `KEY_CONSOLE`); `signal_send` wakes the
target whatever it waits on (`wake_task`). `wake` bumps a global sequence
under `TASKS`, and `block_until` refuses to block when the sequence moved,
so a wake between the check and the block is never lost; spurious wakes are
fine because every caller re-checks. `WAIT_ANY` waiters (`SYS_POLL`,
`SYS_NANOSLEEP` with `SLEEP_ANY_EVENT`, the Linux `ppoll`) are woken by
every wake and by `wake_any` after device/file writes, the last close of a
module file and exits — that is what lets `poll` and `netd` sleep instead of
spinning on `gettimeofday`. `wake_any` skips the task scan when no `WAIT_ANY`
waiter is registered, but bumps the sequence first, and `block_until`
registers a `WAIT_ANY` waiter before it checks the sequence: either the
producer sees the waiter or the waiter sees the bump. (Before, a unix-socket
write landing between a poller's scan and its registration was missed until
some other event: the X server then left a client's requests unanswered.)

`SYS_POLL` (`poll(fds, nfds, timeout_ms)`, `sys_poll`) scans a `struct
pollfd` array with `task::fd_poll` and sleeps on `WAIT_ANY` until the first
fd is ready, the timeout, or a signal. Readiness comes from each object:
pipes and ptys (data, room, the peer gone), the console tty (committed
input; a keyboard without an interrupt is polled, so a watched tty then
re-checks every 10 ms), regular files (always ready), and module files through
the optional `ModuleVfsOps::poll` hook (ABI 17): netfs reports a socket's
bytes, hangup, finished connect, queued accept and, for `/net/unix`, room
in the peer's buffer; a hook that adds `MYOS_POLL_RECHECK` is re-checked
every 10 ms like the tty (`/dev/console/kbd` when its keyboard has no
interrupt, ABI 20),
and a module `read` that returns `MYOS_READ_WAIT` makes an fd read wait for
the file the same way. libgloss's `poll`/`select` are one call, and so is
every blocking wait of its socket library (`pollselect.c`, `socket.c`).

Deadlines use `time::monotonic_ns()` (x86: TSC at the rate CPUID leaf 15H/16H states, else calibrated against the PIT
channel 2 at boot; aarch64: `CNTVCT_EL0`; riscv64: `time` CSR). Each timer
IRQ calls `task::timer_tick()`, which only scans `TASKS` once the earliest
deadline (`NEXT_DEADLINE`) has passed. The x86 LAPIC tick is calibrated
against that clock to 100 Hz (it used to fire 10-20k times per second per
CPU, each tick taking the scheduler lock; at 1 kHz the ticks cost a
self-hosted core+alloc build 10-14% under TCG). All three tick at 100 Hz
but are tickless for deadlines: `block_until` calls
`arch::timer_deadline`, which pulls the CPU's own timer in to the new
deadline (aarch64/riscv64: `cntv_cval_el0` / `stimecmp`, absolute compare
values; x86: the periodic LAPIC timer's current count, cut short and put
back at the tick), and every re-arm programs `min(next tick,
task::next_deadline_ns())`, so a 1 ms
`nanosleep` ends after about 1 ms instead of at the next 10 ms boundary.

Idle CPUs are tickless (issue #367): a CPU that halts with
nothing to run (`sched::halt`, from the idle loop and `block_until`) stops
its tick and arms its timer once, for `NEXT_DEADLINE` or at most 1 s ahead
(`arch::timer_idle`: x86 puts the LAPIC timer in one-shot mode, aarch64
turns the physical timer off and sets `cntv_cval_el0`, riscv64 sets
`stimecmp`), so an idle CPU is not woken 100 times a second. The 1 s
backstop bounds what a wakeup that was never kicked costs. Whatever ends the
halt and runs something puts the tick back (`arch::timer_resume`, from
`halt` and from `schedule`, since an interrupt may switch tasks from inside
the halt): a busy CPU keeps its 100 Hz tick for preemption. CPU 0 also
wakes for the cursor blink (every 500 ms, while the screen shows the text
console), and keeps its tick where the UART has no interrupt (no I/O APIC):
that tick drains the UART then (`input::tick`). Under single-threaded TCG (`MYOS_TCG_SINGLE=1`, `local-ci.sh`) an IPI
to a halted CPU waits for QEMU's 100 ms round-robin kick, since one host
thread runs every vCPU; TLB shootdowns waiting for a tickless CPU's ack are
then that slow, so the boot tests use multi-threaded TCG on every arch. Each idle CPU's halts then last up to its next deadline (the
`idle_tickless` test checks a quiet CPU averages over 20 ms a halt,
`idle_cpu0` the same of CPU 0).

Run queues: the Ready tasks that are on no CPU sit in per-CPU run queues
(`sched::RUNQ`, a bit per task slot: the tasks homed on that CPU) and one
queue of the tasks that may run anywhere (`FLOAT`, the kernel's threads);
the idle tasks are never queued. The bits change with the task states
under `TASKS` (a task is queued when it is made Ready off a CPU: a wake, a
spawn, `finish_switch` for a preempted task), so picking the next task is a
few bit operations on the CPU's queue and the floating one, round-robin
after the current slot, and "is anything ready here?" is a lock-free load
(the old pick walked all 64 slots under `TASKS` on every tick, switch and
yield, and again before every halt).

Idle: `kernel_main` (task 0) is the BSP's idle task, `ap_idle_body` the APs'.
`idle_step` is one pass of the scheduler: the CPU marks itself idle
(`CPU_IDLE`), picks under `TASKS` (its queue, the floating queue, else a
pull, below) and, when that found nothing, halts (`arch::idle_wait`: `sti;
hlt` / `wfi` with the pending-interrupt wakeup) unless `NEED_RESCHED[cpu]`
was set or its queue filled meanwhile: a task queued for an idle CPU after
its pick sets the flag and sends it an IPI, which is pending when it
halts. A task that blocks with nothing else runnable halts the same way on
its own stack, from the same single pass (`block_until`); `schedule` never
picks a woken task that is still some CPU's `CURRENT` (it is not queued),
and a wake that lands while the task is mid-switch (`SWITCHED_FROM`) is
deferred to `finish_switch` (`wake_pending`). A task leaving a CPU stays
Running until `finish_switch`, also one a wake already made Ready while it
halted there: left Ready, a peer could resume it from its previous, stale
switch frame before `task_switch` saved the new one. `wake` queues the
woken task for its home CPU (or marks the CPU it is halting on), and sends
a **targeted** reschedule IPI (`IrqChip::send_ipi` to one CPU: the LAPIC ICR with
a destination, a GIC SGI to one CPU (v2: a CPUTargetList bit, v3: the affinity in `ICC_SGI1R_EL1`), SBI IPI with a single
hart) only when that CPU is halted.

Idle-pull balancing (`sched::pull`): a CPU with nothing in its queues, and
whose current task cannot go on, takes a user task from the queue of a busy
CPU (one that is not idle: an idle CPU runs its own queue as soon as it is
kicked) and makes itself its home. A user task so keeps a home, runs on one
CPU at a time and is loaded on that CPU only (`LOADED_ASPACE`): the task is
off every CPU while it waits, and the CPU it left switched to another root
before `finish_switch` queued it, which flushed its TLB, so the TLB rules
for threads (`docs/threads.md`) cover a pulled task too. A task queued for
a busy home, or a floating task, kicks an idle CPU that may run it
(`kick_for`), so the pull happens at once rather than at the home's next
tick; a kernel thread is never pulled (it floats), nor is a user task onto
the BSP where `user_affinity` keeps user tasks off it (x86_64, aarch64).
`/proc/cpuinfo` counts the pulls per CPU. The UART's receive interrupt stages its
bytes (`input::drain_uart_irq`, into a lock-free ring) and wakes
`KEY_CONSOLE`; the keyboard's only wakes it (`KernelApi::console_input`),
and the reader takes the keys. Without those interrupts the BSP's tick
stages UART RX and the console reader re-polls the keyboard every 10 ms.

The UARTs are programmed once (`SerialPort::new` used to reprogram COM1 /
the PL011 on every output byte; the FIFO-reset bits in that sequence dropped
input that arrived while the kernel echoed, which only showed once the drain
tick slowed down).

`/proc/cpuinfo` reports `schedules`, `idle_halts` and `pulls` per CPU,
`blocked_tasks`, `clock_hz` and `uptime_ms` — an idle shell should show
`idle_halts` climbing and `schedules` nearly flat.

## SMP model per architecture

All three arches use **Limine `MpRequest`**: the bootloader parks APs until
`MpInfo::bootstrap(ap_entry, logical_cpu)`. APs stay online into userspace.

| Arch | CPU id | AP init | Timer / IRQ | IPI |
|------|--------|---------|-------------|-----|
| x86_64 | TSC_AUX / APIC id | Per-CPU GDT+TSS, GS → syscall state, LAPIC timer (x2APIC MSRs when the CPU has an x2APIC, the xAPIC page otherwise; QEMU's TCG has no x2APIC, so CI runs the xAPIC path) | LVT timer → `schedule` | ICR all-excl-self (vec 33 TLB, 34 resched) |
| aarch64 | `TPIDR_EL1` / `MPIDR_EL1` | naked `goto_address` entry, the kernel TTBR0 and MAIR device attribute, `VBAR`/`use_spx`, the CPU's GIC bank (v2: banked GICC; v3: its redistributor + `ICC_*`), timers | PPI timer → `schedule` | GICv2 SGI 0 (TLB), SGI 1 (resched) |
| riscv64 | `tp` / Limine `hartid` | `stvec` / `sie` (STIE+SSIE) / `stimecmp` | S-mode timer → `schedule` | SBI IPI ext → SSIP; soft reason bits in `smp` |

Scheduler: per-CPU run queues + optional `affinity` (AP idle threads are
pinned; a user task's home moves when an idle CPU pulls it, above).
Kernel tasks use `affinity: None` (smoke `sched mask=0x3`). With more than one
CPU online, `spawn_user` round-robins user homes: **x86_64** and **aarch64**
over the APs (skip BSP), **riscv64** (`-smp 2`) over both harts. Fork
inherits the parent home for sequential fork+exec+wait and for init→getty
(init has no ctty; its long-lived `netd` child must not look like `make -j`).
If a **ctty-bearing** parent already has a Ready/Running child (interactive
`make -j` / pipelines), the new child takes a fresh AP RR home instead. Exec
keeps that affinity — no basename sticky/rehome allowlists and no blanket
post-exec RR (blanket re-home burned the UEFI 600s QEMU wall under CI; plain
`parent_has_active_child` without the ctty gate RR-spread getty and UEFI
page-faulted after fork exec). Cross-CPU exit reclaim is kept safe by: (1)
`die` enabling IRQs before reclaim/TLB shootdown, (2) epoch-based TLB
shootdown with soft `tlb_service` from `schedule` while IF-off (IRQ-only ACK
previously deadlocked a cli waiter), (3) x86 `flush_user_tlb` / unload skip
the remote IPI barrier when affinity pin guarantees the aspace was never
loaded elsewhere — full shootdowns after every map/unmap doubled MYOS_CI_MINI
under `-smp 4` TCG (600s wall); do not paper with a longer QEMU timeout.
A user task moves CPU only while it is Ready and off every CPU (a pull),
never while running (`affinity: None` user tasks stay off: the NX #PF /
leave races). `schedule` switches aspace/rsp0 before
publishing Ready (old stays Running across the CR3 write, lock not held during
switch); `unload_user_aspace` briefly kicks remotes then TLB-shootdowns; fork
queues the child for its home and kicks that CPU, or an idle one that may
pull the child while the parent keeps the home busy.
**aarch64** user tasks were BSP-pinned while block I/O waited on BSP-targeted
SPIs; virtio-blk/NVMe are polled now, so they take AP homes like x86 (the
`tlbi …is` flushes are inner-shareable broadcasts anyway). **riscv64** APs
join the scheduler too (`RISCV_PARK_APS = false`): the U-mode trap entry
reloads `tp` from the CPU-id footer the scheduler stamps at the base of the
task's kernel stack, so `cpu_id()` is right on every hart even though user
TLS lives in `tp`; the `sepc=0` corruption that once forced the WFI park was
the trap vector's t0 clobber (fixed in #179). The aarch64/riscv64 syscall
frame pointer used by fork/exec is a per-CPU cell (`SYSCALL_FRAMES`) that
`schedule` saves with the outgoing task and restores for the incoming one,
so a syscall that blocked and resumes (on any CPU) finds its own frame.
`note_schedule` → `/proc/cpuinfo`. QEMU `-smp 4` on x86/aarch64 (interactive
+ CI); riscv stays `-smp 2` (Limine panics `missing struct riscv_hart for
BSP` at 4). The packed `boot/virt.dtb` is always dumped with `-smp 2` so
OpenSBI BSP hartid=1 still has a hart node. The WFI park stub stays compiled
for riscv64 (flip `RISCV_PARK_APS` to retreat).
On every arch the BSP timer drains UART RX into a staging ring
(`input::drain_uart_irq`) and wakes the console reader (`KEY_CONSOLE`), so a
starved shell under `-smp 4` TCG cannot overrun the COM1 FIFO / sticky-key
login and a blocked reader never polls the UART itself.

Per-CPU ring3↔ring0 state:

- **x86_64** — each CPU has its own GDT+TSS (`rsp0` / DF IST). `IA32_EFER.NXE`
  and `IA32_GS_BASE` → `CpuSyscallState` are programmed on BSP and every AP
  (`kernel_rsp0` + fork callee snapshot). User CS/SS come from the CPU's GDT.
- **aarch64** — banked `SP_ELx` after `use_spx`; exception frames live on the
  current task's kernel stack. Limine `goto_address` publishes via STLR+DC CVAC;
  APs take a naked entry (FPEN + progress flag) then install the kernel TTBR0 and the MAIR device attribute before the GIC (the device window is in the shared TTBR1).
- **riscv64** — `sscratch` holds the current task's kernel stack top (updated on
  every schedule). Per-hart cells are deferred until multi-hart Limine bring-up
  is reliable on QEMU.

TLB shootdown: local invalidate, then IPI the other online CPUs and wait
briefly for acks (`smp::tlb_shootdown`, try-lock + bounded spin). An idle
CPU (in the idle loop, or halted) is passed over: it runs no user code, so
it is flagged instead (`TLB_STALE`) and flushes its TLB when it next
resumes a task (`schedule`), and its ack is given for it. Waiting for a
halted CPU's ack cost its wake per shootdown (under TCG the host thread's),
for every copy-on-write fork and exit of a shell command. A targeted
reschedule IPI wakes a halted CPU when a task homed there becomes Ready
(`spawn`, `fork`, `wake`).

AP bring-up: IRQs stay masked and `ONLINE` is clear until `ap_idle_loop`
installs `CURRENT` and migrates onto the AP's own 64KiB idle stack (Limine
AP stacks are too small for nested timer/IPI frames).

## `/proc` nodes

- `/proc/mounts` — existing
- `/proc/cpuinfo` — online CPUs, hw ids, schedule counts
- `/proc/platform` — the board description and where each component came from (above)
- `/proc/usb` — the controllers (ports, interrupt, event counts) and the USB devices: id, parent hub and port, speed, vendor:product, each interface's class triple and the driver that took it (`docs/usb.md`)
- `/proc/pci` — full BDF list from `pci_enum` (hex IDs + class/subclass names and a small QEMU/virt device table). Write `rescan` to re-enumerate and refresh the node (gone devices disappear); the kernel then calls every module's `module_rescan`, and the block drivers (virtio-blk, virtio-scsi, NVMe) bring up the disks that appeared since boot, leaving the known ones alone. virtio-net probes once at load (netd binds the one `/dev/net0`). No ACPI/QEMU hotplug IRQ yet: a hot-added disk shows up after a rescan.
- `/proc/acpi/info`, `tables`, `s5` — from `acpi` module (honest stubs if no RSDP)

## Out of scope

ACPICA, full OSPM/sleep, Wayland, guest rustc.
