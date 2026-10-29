# Interrupts

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
| riscv64 | `aia`: the APLIC and the IMSICs (with the SBI's IPIs; the MSI controller too, "MSI controllers" below) | `riscv,aplic` + `riscv,imsics` (the supervisor level's) | — | `arch/riscv64/aia.rs` |

An MSI controller, the GICv3's ITS or a GICv2m frame, is not the root:
`irqchip::MsiChip` ("MSI controllers" below), which the platform
description names (`msi its ...`).

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
  domain's `unmask` with the line, and its `mask` when `irq_disable`
  takes the line's last handler away (ABI 56);
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
the kernel, its handler quietens the line, and a thread of the module
gives the interrupt back (`irq_disable`), so the kernel masks the line
through the domain and the line asserted again interrupts nobody
(`/proc/pl061`: `check given back`; `/proc/interrupts`: `pl061`, the
controller's; the `irq_domain` test).

## Device interrupts

`kernel/src/irq.rs` keeps one handler per interrupt number and `pci_irq_setup`
picks the delivery mechanism per arch, so a module only asks for "this PCI
function's interrupt" (`KernelApi::pci_irq_enable`) and gets told whether to
use an MSI-X table entry or the INTx line:

| Arch | Mechanism | Numbering |
|------|-----------|-----------|
| x86_64 | MSI-X entry 0 → LAPIC vector on the BSP (`DEVICE_VECTOR_BASE` 48 + n, 64 vectors shared with the I/O APIC pins below and the IOMMU's own interrupt; no PIRQ routing; the message through the IOMMU's interrupt remapping where the board has one, `iommu::remap_msi`, "The IOMMU" below: a BSP with an x2APIC id above 255 is reached only that way, in extended interrupt mode, and gets no MSI otherwise); a function without MSI-X gets its one MSI message the same way (`MYOS_IRQ_MSI`); a function with neither its INTx line through an I/O APIC pin, level and active low as the firmware's `_PRT` says ("AML" below; the pin's vector shared by the functions on it, remapped like the ISA pins; the line's other functions, a driver's that polls or is on MSI, have their INTx turned off, the command register's disable bit, so none holds the level line asserted), or none when the acpi module gave no route for it (its driver polls) | vector |
| aarch64 | MSI-X entry 0, else MSI, through the board's MSI controller ("MSI controllers" below): a GICv3's ITS (an LPI) or a GICv2m frame (an SPI, edge-triggered); without one, or with its interrupts all taken, INTx → GIC SPI, level, priority 0x80, delivered to the BSP (GICv2: `ITARGETSR` CPU 0; GICv3: `IROUTER` to its affinity); the SPI comes from the device tree's PCIe `interrupt-map` (QEMU `virt`: SPI 3 + (slot + pin − 1) mod 4) | INTID (35..38 for INTx; the GICv2m frame's SPIs, QEMU's 80..143; LPIs from 8192) |
| riscv64 | MSI-X entry 0, else MSI, through the AIA's IMSIC (an identity of the boot hart's interrupt file, "MSI controllers" below); else, and on a board with a PLIC, INTx → the PLIC source for the boot hart's S-mode context, or the AIA's APLIC source, from the same `interrupt-map` (`virt`: 32 + (slot + pin − 1) mod 4), `sie.SEIE` | PLIC or APLIC source (an IMSIC identity of the same number); an MSI's identity above the APLIC's sources |

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

## MSI controllers

On aarch64 and riscv64 a PCI function's MSI-X or MSI is programmed the
same way as on x86_64 (`kernel/src/msi.rs`: the table entry or the
capability, a 64-bit address where the function takes one) with the
message the board's MSI controller gives (`irqchip::MsiChip`, started
after the root controller, `irqchip::start_msi`); a function without
either, or a board without a controller, keeps its INTx. A function's
interrupt given back (`KernelApi::pci_irq_disable`) frees the
controller's interrupt.

| Controller | Arch | What a message is | Described by |
|---|---|---|---|
| GICv3 ITS (`arch/aarch64/gic_msi.rs`) | aarch64 | the event id written to `GITS_TRANSLATER`, the device id the requester id through the host's `msi-map`: an LPI (256 from 8192, priority 0xa0) of collection 0, the BSP's redistributor. The kernel gives the ITS flat device (12 bits of device id) and collection tables and a page of commands (`MAPD` a device's translation table of 8 events, `MAPC`, `MAPTI`, `INV`, `SYNC`, `DISCARD`), the redistributor its LPI configuration and pending tables (14 bits of interrupt id), each written back for an ITS that does not snoop | the MADT's GIC ITS entry, the tree's `arm,gic-v3-its` node the PCIe host's `msi-map` names: `/proc/platform`'s `msi its ...` line |
| GICv2m frame (`gic_msi.rs`) | aarch64 | an SPI's INTID written to the frame's `MSI_SETSPI_NS`: the SPI, edge-triggered, to the BSP (QEMU's frame: 64 from 80) | the MADT's GIC MSI frame entry, the tree's `arm,gic-v2m-frame` node (`arm,msi-base-spi`, `arm,msi-num-spis`, else the frame's `MSI_TYPER`): `msi v2m ...` |
| RISC-V AIA (`arch/riscv64/aia.rs`) | riscv64 | an identity written to the boot hart's supervisor-level IMSIC page. The AIA is the board's root controller: the APLIC (its supervisor domain, MSI delivery mode) turns wired source `n` into identity `n`, so the tree's numbers stay the `irq::dispatch` numbers, a level-triggered source sent again at its end while still asserted; the identities above the sources are the MSIs. The boot hart's interrupt file has every identity enabled (its CSRs are its own: `siselect`/`sireg`, the claim through `stopei`), the other harts' are off; the IPIs stay the SBI's | the tree's supervisor-level `riscv,aplic` and `riscv,imsics` nodes (the machine level's are the firmware's), the hart's place in the IMSICs' `interrupts-extended`: `intc aia ...`, `msi imsic ...` |

QEMU `virt` has the ITS with `gic-version=3` and the GICv2m frame with a
GICv2 (both aarch64 machines of CI, the boot lines `[ OK ] msi: its, 256
LPIs from 8192` and `[ OK ] msi: gicv2m, SPIs 80-143`), and the AIA with
`aia=aplic-imsic`, which the launcher's riscv64 machine is
(`MYOS_RISCV_INTC`: `aia`, the default, or `plic` for the PLIC alone,
INTx only; CI boots both, `riscv64` and `riscv64-plic`; the boot line
`[ OK ] msi: imsic, identities 97-255`). The Raspberry Pi 3 has no PCI.
