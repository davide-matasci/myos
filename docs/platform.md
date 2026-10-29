# The platform description and ACPI

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
Both have an MSI controller for the PCI devices: the GICv3 its ITS, the
GICv2 a GICv2m frame ("MSI controllers" below).

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
before the GIC/PLIC EOI (on x86_64 the LAPIC's, which a level pin of the
I/O APIC waits for). An INTx line is shared by every slot the swizzle
puts on it (QEMU `virt` has four for all of them): each driver of a device
on it registers its handler, and every one runs at each interrupt of the
line, its device saying whether it was the one. `/proc/interrupts` lists
counts per registered handler plus spurious interrupts, and on x86_64 the
device vectors taken (`vectors: N/64`). The kernel holds 64 handlers on
every arch, as many as x86_64 has device vectors.

A PCI function's interrupt is given back with `KernelApi::pci_irq_disable`
(ABI 55, `irq::pci_irq_disable`): the handler leaves the table (64 slots)
once no CPU runs it, the function's interrupt is turned off (its MSI-X or
MSI, or its INTx: the command register's disable bit), and once the
interrupt has no handler left what the arch took for it is free for the
next device. On x86_64 that is the device vector (64 of them, a bitmap,
the lowest free one given out), the IOMMU's interrupt remapping entry
(VT-d's or AMD-Vi's, 256 a table, the lowest free one reused) and, for an
INTx line, its I/O APIC pin; on aarch64 and riscv64 an MSI's controller
interrupt (an ITS LPI, a GICv2m SPI, an IMSIC identity), while the
INTx lines (GIC SPIs, PLIC and APLIC sources) are not allocated, only
their handlers, and a line with no handler left is masked (another
slot's device may still be on a shared INTx line). A board device's interrupt is given back with
`KernelApi::irq_disable` (ABI 56, `irq::disable`): the handler leaves the
table the same way and the line is masked once no handler is left on it,
through the controller (`IrqChip::mask`: a GIC SPI's `ICENABLER`, the
Raspberry Pi's disable registers, the PLIC's or the APLIC's enable bit, an ISA pin of
the I/O APIC, which keeps its vector for its next enable) or the domain's
`mask`. Either call lowers the registration count of the module that
enabled the interrupt. `/dev/pci` gives a function's interrupt back when
its driver closes `ctl`; the kernel's own drivers keep theirs.

virtio-net uses this for its RX queue: `AVAIL_F_NO_INTERRUPT` is cleared on
the RX ring only, the handler acks the ISR and calls `wake_any`, which ends
the `poll` `netd` sleeps in on `/dev/net0/data` and its netfs channel (the
kernel's poll reads the wait sequence before it asks the module's readiness
hook, so a frame that lands between the check and the sleep is not missed).
The NIC's `ctl` says `irq on` when that works; on `irq off` (a poll-mode
device) `netd` keeps its 2–10 ms timed polling. Reads of `/dev/netN/data`
stay non-blocking.

## AML

On x86_64 the acpi module runs the firmware's AML with the `acpi` crate's
interpreter (`modules/acpi/src/aml.rs`, the crate used unmodified over
the module ABI: memory through `phys_map`, the I/O ports, PCI
configuration space). It calls `\_PIC(1)` (the OS uses the APIC), runs
the devices' `_STA` and `_INI`, then reads every PCI bridge's `_PRT`
(through its link devices' `_CRS` where the firmware has them) and hands
each route to the kernel (`KernelApi::pci_intx_route`: bus, device, pin,
GSI, trigger and polarity). A root bridge (`_HID` or `_CID` `PNP0A03`,
`PNP0A08`) is on its `_BBN`'s bus; a PCI-to-PCI bridge's routes are for
its secondary bus. A function behind a bridge with no `_PRT` of its own
is found through the PCI-to-PCI swizzle (`(pin + device) mod 4` per
bridge, `kernel/src/arch/x86/intx.rs`). It also evaluates `\_S5` for the
power-off. `/proc/acpi/aml` reports what it found:

```
prt \_SB.PCI0 bus 0 routes 128
routes 128
_S5 0 0
```

The interpreter is young and stops at what it does not implement (a
panic): it runs in a thread of its own, which a panic parks, and the
module's init waits for it 5 seconds at most. Then nothing is routed
(the drivers of functions without MSI poll, as before) and `_S5` comes
from the scan below; `/proc/acpi/aml` says why, and `/proc/acpi/s5`'s
`source:` which one gave it (`AML interpreter`, `AML _S5 scan`).

### The `_S5` scan

Used on the other arches, and on x86_64 when the interpreter failed, to
resolve `Name (_S5_, Package …)` in the DSDT for `/proc/acpi/s5`
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
