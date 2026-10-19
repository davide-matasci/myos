# Raspberry Pi

myos runs on a **Raspberry Pi 4** behind its community UEFI firmware
([pftf/RPi4](https://github.com/pftf/RPi4)): the firmware starts Limine
from a USB stick like any UEFI machine, and the kernel reads the board
from the firmware's device tree. **Untested on hardware so far**: the
kernel's side is checked on QEMU `virt` with a device tree shaped like
the Pi's (the UART and GIC behind a `simple-bus` whose `ranges` move
them, disabled decoy nodes, `stdout-path` through an alias), but no QEMU
model runs the Pi's firmware, so no CI boot covers it. The Pi 5 is
not supported yet.

What should work: the four cores, the GIC-400, the generic timer, the
serial console, the HDMI framebuffer the firmware sets up (`docs/fb.md`),
everything that runs from the initramfs, and USB (`docs/usb.md`): a
keyboard on the HDMI console, a mouse, memory sticks, hubs, through the
VL805 xHCI controller behind the BCM2711's PCIe bridge. What does not
(yet): the SD card; Ethernet (GENET); Wi-Fi; the RTC (the Pi has none).

## Building the image

```sh
cargo run -- rpi4        # writes target/rpi4.img
```

`target/rpi4.img` is the AArch64 boot disk with two differences from
`target/aarch64.img`: Limine is not given QEMU's device tree (so the
kernel gets the firmware's), and the kernel command line has
`platform=dt` (the firmware can offer ACPI tables too; myos reads the
device tree, `docs/pci-acpi-smp.md`). Write it to a USB stick:

```sh
sudo dd if=target/rpi4.img of=/dev/sdX bs=4M conv=fsync
```

## The firmware

1. Format a microSD card with one FAT32 partition and unzip the latest
   [pftf/RPi4 release](https://github.com/pftf/RPi4/releases) onto it.
2. Boot the Pi with it once, press Esc for the setup and set **Device
   Manager → Raspberry Pi Configuration → ACPI / Device Tree → System
   Table Selection** to **Devicetree** (or **ACPI+Devicetree**). Save and
   reset.
3. With the USB stick plugged in, the firmware boots it
   (`EFI/BOOT/BOOTAA64.EFI`, Limine), or pick it in the Boot Manager.

## Serial console

A 3.3 V USB serial adapter on GPIO 14 (TX, pin 8) and 15 (RX, pin 10),
ground on pin 6, at 115200 8N1. The kernel uses the UART the device
tree's `/chosen` `stdout-path` names: the PL011 when the firmware's
`config.txt` has `dtoverlay=miniuart-bt` (pftf's does), else the mini
UART. It keeps the firmware's baud rate on a PL011 it finds enabled (the
Pi's runs from a 48 MHz clock, not the 24 MHz the kernel's own divisors
assume).

## How the kernel finds the board

`kernel/src/dt.rs` skips nodes whose `status` is not `okay` and
translates each `reg` through the `ranges` of the buses above it (the
Pi's peripherals are at bus address `0x7e00_0000`, CPU address
`0xfe00_0000`). Device MMIO is mapped in the kernel's higher-half window
(`kernel/src/arch/aarch64/paging.rs`), so the Pi's devices near 4 GiB
do not collide with user space. A board without a PCIe host bridge the
kernel can drive boots without PCI.

## PCIe and USB

The BCM2711's PCIe bridge (`brcm,bcm2711-pcie`) is not ECAM: its
registers show the root port's configuration space at offset 0, and one
other function's at a time through a window at 0x8000, chosen by writing
`bus << 20 | slot << 15 | func << 12` at 0x9000
(`kernel/src/arch/aarch64/pci.rs`; the register layout as FreeBSD's
`bcm2838_pci.c` documents it). The kernel uses the bridge as the UEFI
firmware left it, which brought it up for its own USB boot: the link
trained (bits 4 and 5 of the state register at 0x4068), the windows set,
the root port's bus numbers assigned, and the VL805's firmware loaded.
It scans the buses the root port's subordinate number covers, slot 0
only on buses 0 and 1 (addressing another slot on bus 0 hangs the
memory controller). The 32-bit window is at CPU `0x6_0000_0000`, bus
`0xc0000000`: BARs hold bus addresses, mapped at their CPU address.

The VL805 reaches the low 3 GiB (the bridge's `dma-ranges`), and its DMA
does not see the CPU's caches: the xHCI driver's DMA memory comes from
there, whatever the firmware's "Limit RAM to 3 GB" setting, and it
cleans and invalidates the caches around each hand-over (`dma_sync`,
`docs/pci-acpi-smp.md` "DMA").
