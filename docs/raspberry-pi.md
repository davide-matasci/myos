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

What works: the four cores, the GIC-400, the generic timer, the serial
console, the HDMI framebuffer the firmware sets up (`docs/fb.md`), and
everything that runs from the initramfs. What does not (yet): USB (the
Pi 4's VL805 xHCI sits behind Broadcom's own PCIe host bridge, which is
not ECAM), so no keyboard and no USB disks once booted; the SD card;
Ethernet (GENET); Wi-Fi; the RTC (the Pi has none). The shell is on the
serial console.

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
