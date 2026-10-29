# Raspberry Pi

myos runs on a **Raspberry Pi 4** and boots a **Raspberry Pi 3** to a
serial shell ("Raspberry Pi 3", further down). The Pi 5 is not supported
yet.

The Pi 4 runs behind its community UEFI firmware
([pftf/RPi4](https://github.com/pftf/RPi4)). The firmware starts Limine
from a USB stick like any UEFI machine, and the kernel reads the board
from the firmware's device tree.

The Pi 4 is **untested on hardware so far**. The kernel's side is checked
on QEMU `virt` with a device tree shaped like the Pi's: the UART and GIC
behind a `simple-bus` whose `ranges` move them, disabled decoy nodes, and
`stdout-path` through an alias. No QEMU model runs the Pi's firmware, so
no CI boot covers the Pi 4.

What should work on the Pi 4:

- the four cores, the GIC-400, the generic timer and the serial console;
- the HDMI framebuffer the firmware sets up (`docs/fb.md`);
- everything that runs from the initramfs;
- USB (`docs/usb.md`) through the VL805 xHCI controller behind the
  BCM2711's PCIe bridge: a keyboard on the HDMI console, a mouse, memory
  sticks, hubs;
- the SD card (below);
- Wi-Fi: the CYW43455 as `/dev/sdio`, driven by the `wifi` daemon
  ("Broadcom's radios" in `docs/wifi.md`).

What does not work yet: Ethernet (GENET).

The Pi has no RTC. Its wall clock counts from the epoch at boot until
`date` or `settimeofday` sets it.

## Building the image

```sh
cargo run -- rpi4        # writes target/rpi4.img
```

`target/rpi4.img` is the AArch64 boot disk with two differences from
`target/aarch64.img`:

- Limine is not given QEMU's device tree, so the kernel gets the
  firmware's.
- The kernel command line has `platform=dt`. The firmware can offer ACPI
  tables too, but myos reads the device tree (`docs/platform.md`).

Write it to a USB stick:

```sh
sudo dd if=target/rpi4.img of=/dev/sdX bs=4M conv=fsync
```

## The firmware

1. Format a microSD card with one FAT32 partition and unzip the latest
   [pftf/RPi4 release](https://github.com/pftf/RPi4/releases) onto it.
2. Boot the Pi with it once and press Esc for the setup. Set **Device
   Manager → Raspberry Pi Configuration → ACPI / Device Tree → System
   Table Selection** to **Devicetree** (or **ACPI+Devicetree**). Save and
   reset.
3. With the USB stick plugged in, the firmware boots it
   (`EFI/BOOT/BOOTAA64.EFI`, Limine). You can also pick it in the Boot
   Manager.

## Serial console

Connect a 3.3 V USB serial adapter to GPIO 14 (TX, pin 8) and 15 (RX,
pin 10), with ground on pin 6, at 115200 8N1.

The kernel uses the UART that the device tree's `/chosen` `stdout-path`
names. That is the PL011 when the firmware's `config.txt` has
`dtoverlay=miniuart-bt` (pftf's does), else the mini UART. On a PL011 it
finds enabled, the kernel keeps the firmware's baud rate: the Pi's PL011
runs from a 48 MHz clock, not the 24 MHz the kernel's own divisors
assume.

## How the kernel finds the board

`kernel/src/dt.rs` skips nodes whose `status` is not `okay`. It
translates each `reg` through the `ranges` of the buses above it: the
Pi's peripherals are at bus address `0x7e00_0000`, CPU address
`0xfe00_0000`.

Device MMIO is mapped in the kernel's higher-half window
(`kernel/src/arch/aarch64/paging.rs`), so the Pi's devices near 4 GiB do
not collide with user space. A board without a PCIe host bridge the
kernel can drive boots without PCI.

## PCIe and USB

The BCM2711's PCIe bridge (`brcm,bcm2711-pcie`) is not ECAM. Its
registers show the root port's configuration space at offset 0, and one
other function's at a time through a window at 0x8000. Writing
`bus << 20 | slot << 15 | func << 12` at 0x9000 chooses the function
(`kernel/src/arch/aarch64/pci.rs`; the register layout is the one
FreeBSD's `bcm2838_pci.c` documents).

The kernel uses the bridge as the UEFI firmware left it. The firmware
brought it up for its own USB boot:

- the link trained (bits 4 and 5 of the state register at 0x4068);
- the windows set;
- the root port's bus numbers assigned;
- the VL805's firmware loaded.

The kernel scans the buses the root port's subordinate number covers. On
buses 0 and 1 it scans slot 0 only, because addressing another slot on
bus 0 hangs the memory controller. The 32-bit window is at CPU
`0x6_0000_0000`, bus `0xc0000000`: BARs hold bus addresses, and the
kernel maps them at their CPU address.

The VL805 reaches the low 3 GiB (the bridge's `dma-ranges`), and its DMA
does not see the CPU's caches. So the xHCI driver's DMA memory comes from
the low 3 GiB, whatever the firmware's "Limit RAM to 3 GB" setting, and
the driver cleans and invalidates the caches around each hand-over
(`dma_sync`, `docs/pci.md` "DMA").

## The SD card

The `sdmmc` module (`modules/sdmmc`) makes the card `/dev/mmcblk0`, a
disk like any other, with its partitions as `/dev/mmcblk0/pN`
(`docs/install.md`). Its card layer brings the card up and moves the
blocks, behind the controller's driver. The card layer uses the SD
Physical Layer Specification's commands and registers, from the
`sdio-host` crate.

The controllers:

- **Pi 4: the EMMC2** (`brcm,bcm2711-emmc2`), a standard SDHCI
  controller. The same driver is checked with QEMU's `sdhci-pci` on every
  arch (the `sdmmc` test); it is untested on the Pi 4 itself. Every
  register access is 32 bits wide, because the Broadcom controllers lose
  narrower writes. Its base clock is the one the firmware reports
  (`rpi_firmware`, the EMMC2 clock), else its capabilities' value, else
  250 MHz is assumed, so the card never runs faster than asked.
- **Pi 3: the BCM2835's own SD host controller**
  (`brcm,bcm2835-sdhost`). Broadcom does not document it; the driver uses
  its registers as Linux's `bcm2835` MMC driver and QEMU's model do. It
  runs off the core clock, which the firmware reports. Without the
  `rpi_firmware` module the divisors assume the highest core clock,
  400 MHz (250 MHz with `enable_uart=1`). Its pins stay as the firmware
  set them. The Pi 3's SDHCI controller (`brcm,bcm2835-sdhci`) is its
  Wi-Fi's: an SDIO card, below.

Each controller takes a single card, on the 4-bit bus. The card is
switched to high speed (CMD6, 50 MHz) when its CSD says it takes the
switch and the controller can run it: on SDHCI, the capabilities and the
host control's high-speed bit; the sdhost runs it by the clock alone, as
Linux does. Otherwise the card stays at the default speed (25 MHz).

How the blocks move:

- On SDHCI, by ADMA2 through a 64 KiB bounce buffer the controller
  reaches (`dma_alloc`, synced with `dma_sync`, because the Pi's DMA is
  not cache-coherent).
- On the sdhost, through its FIFO by the CPU, as many words at a time as
  it holds. Its DREQ to the DMA engine and the UHS-I modes are issue
  #576.

A wait for the controller (a transfer, a write's end) spins a moment,
then sleeps on its interrupt, with a short nap at most should the
interrupt not come:

- for `sdhci-pci`, `pci_irq_enable` with its INTx (on x86_64 as the
  firmware's `_PRT` routes it), else the waits poll;
- for the device tree's controllers, `irq_enable`.

A card still programming a write is asked once a millisecond, sleeping in
between.

`/dev/mmcblk0/info` ends with `speed high|default clock HZ dma
adma2|none irq on|off`, which the `sdmmc` test checks.

### SDIO: `/dev/sdio`

An SDIO card is not a disk. The module brings it up with the `sdio`
crate (`modules/sdmmc/sdio`): CMD5 for its functions and voltage, the
relative address, the CCCR for the 4-bit bus and high speed, and the CIS
for what it is. The crate is host-tested against a model
(`cargo test -p sdio`). The module then hands the card to a driver that
is a program, as `/dev/sdio/N`, the way `/dev/pci` hands over a PCI
function.

The Pi 3's and Pi 4's Arasan SDHCI (`brcm,bcm2835-sdhci`, the tree's
`mmcnr`) carries the Wi-Fi chip: a CYW43438 on the Pi 3B, a CYW43455 on
the 3B+ and the 4. The module first raises the chip's `WL_REG_ON` through
the firmware's GPIO expander (`rpi_firmware`: `WL_ON`, line 1, held low
then high; `/proc/firmware` shows it). QEMU's firmware has no expander,
and QEMU's controller no card.

A slot whose SD card does not answer is tried as SDIO too (one day,
QEMU's `sdhci-pci` with an SDIO model could test this). SDIO is untried
on hardware so far, like the rest of the Pi 4.

| File | What |
|------|------|
| `info` | `id MMMM:PPPP` (the CIS's manufacturer and product: `02d0:a9bf` a CYW43455, `02d0:a9a6` a CYW43438), `functions N`, `bus 1\|4`, `clock HZ`, `controller sdhci mmio 0x...`, `block F N` for each function with a block size, `irq on\|off\|none` (the card's interrupt at the controller; `none`: no interrupt line), `held` while a program drives it |
| `ctl` | held by one program at a time (root only) from its `open` to its last close; reads as `info`; takes `blocksize F N` (function F's block size, into its FBR, for block-mode transfers) and `irq on` / `irq off`. The files below are there while it is held; letting it go masks the interrupt |
| `fN` | function N's registers, the file position the register address (17 bits): a read or write of one byte is a CMD52, of more a CMD53 with the address incrementing (byte mode up to 512 bytes, block mode for a whole number of the function's blocks, more than one; 64 KiB at most) |
| `fNfifo` | the same at a fixed address (a FIFO) |
| `irq` | a read waits for the card's interrupt and returns how many came since the last read (8 bytes, little endian), `poll` says one is pending; the interrupt stays masked at the controller from the one that fired until a write to `irq` (a byte at least), once the driver quietened the card. The card's own enables (the CCCR's `IENx`) are the driver's, through `f0` |

The CPU moves every transfer through the controller's buffer, so a
program reaches no memory through the card. The `wifi` daemon drives the
Pi's chip this way (`docs/wifi.md`, "Broadcom's radios"). The `sdio` test
checks each card's `info`.

## The VideoCore firmware

The GPU's firmware owns the board's power domains and clocks. The ARM
asks it through the **mailbox property interface** (`brcm,bcm2835-mbox`,
channel 8): a buffer of tags in memory, which the firmware reads and
answers in place. The buffer is handed over by its bus address: the
GPU's uncached alias of memory, which the tree's `dma-ranges` give the
`dma_alloc` of the mailbox's node.

The `rpi_firmware` module (`modules/rpi_firmware`) does this and
publishes it as the service `rpi_firmware`, a `RpiFirmwareOps` table
(`modules/abi`):

- `property` runs a tag list;
- the helpers `power_on` (a domain, such as the USB controller's,
  `RPI_POWER_USB_HCD`) and `clock_rate` (`RPI_CLOCK_CORE`,
  `RPI_CLOCK_EMMC`, ...) wrap the common tags.

The `dwc2` module powers the USB controller through it.

`/proc/firmware` shows what the firmware says of the board: the
firmware's and board's revisions, the EMMC, UART, ARM and core clocks,
and whether the USB controller is powered.

QEMU's `raspi3b` models the property channel, with the firmware's
default clocks, so the Pi 3's CI boot checks the module's path (the
`rpi_firmware` test). A board without the mailbox has no firmware to talk
to: the module says `rpi_firmware none` and publishes nothing.

## Raspberry Pi 3

A Raspberry Pi 3 (BCM2837, four Cortex-A53) has no UEFI firmware. Its
GPU firmware starts an ARM stub and a kernel from the SD card. myos uses
[U-Boot](https://u-boot.org/) as that kernel, and U-Boot runs Limine as
a UEFI application.

QEMU models the board (`raspi3b`), so this boot is checked there, by CI
too (the smoke tests it can run, `docs/testing.md` "In CI"). It is
**untested on hardware so far**.

```sh
cargo run -- rpi3        # builds the boot files and boots QEMU raspi3b
cargo run -- rpi3 test --tags smoke --skip partitions,ext2_disk,fstab_mount,fileio,boot_rc,heap_mini
                         # CI's boot test of it
```

The kernel's console (the mini UART) is on stdio. U-Boot's console (the
PL011) goes to `target/rpi3/u-boot.log`.

What works:

- the four cores, the generic timer and the serial console;
- the framebuffer the firmware sets up;
- everything that runs from the initramfs;
- power-off and reboot;
- the SD card (above);
- USB through the DWC2 controller (below): a keyboard, a mouse, a stick;
- untried, like the Pi 4's: Wi-Fi, the 3 B's CYW43438 or the 3 B+'s
  CYW43455 as `/dev/sdio`, driven by the `wifi` daemon ("Broadcom's
  radios" in `docs/wifi.md`).

What does not work yet: Ethernet. The LAN9514 behind the USB hub needs a
class driver.

The Pi 3 has no RTC either: the wall clock counts from the epoch at boot.

### USB: the DWC2 controller

The Pi 3's USB is Synopsys's DesignWare USB 2.0 OTG core
(`brcm,bcm2708-usb`, one root port, 8 host channels, interrupt GPU 9).
The `dwc2` module (`modules/dwc2`) drives it as a host. It is a
controller driver of the USB core (`docs/usb.md`): it implements the
`UsbHcdOps` the core calls, and everything above it is the same as on
xHCI.

The module brings the controller up:

1. it powers the controller through the firmware (`rpi_firmware`,
   above), when that module is there;
2. resets the core;
3. selects the UTMI+ PHY and host mode;
4. sizes and flushes the FIFOs;
5. turns buffer DMA on.

A transfer takes a channel. The channel's registers say the device, the
endpoint, the packets, and where in memory (`HCDMA`, a bounce page of the
module's, since the Pi's DMA does not see the caches). The channel halts
into its record (`HCINT`), and the interrupt handler wakes the waiter.
These steps run in task context:

- the stages of a control transfer;
- a NAKed interrupt endpoint's retry, asked again after its interval from
  the core's thread (`poll`);
- the halves of a split transaction.

A low- or full-speed device behind a high-speed hub goes through split
transactions. On a real Pi 3 that is every keyboard and mouse, behind the
LAN9514's hub. `HCSPLT` names the hub's address and port; a start half
goes out, then the complete half repeats until the translator has the
answer, one packet per transaction set. QEMU's hub is full-speed and the
root port takes it natively, so CI does not exercise the split path.

Data toggles are kept per endpoint, from the packets each transfer moved.
Bulk transfers go through a 64 KiB bounce buffer, and a control
transfer's data stage is 1 KiB at most.

`cargo run -- rpi3` attaches QEMU's hub to the root port, with the stick,
the mouse and (in a test boot) the serial adapter behind it, as the virt
boards have them on their xHCI. The `usb_disk`, `mouse`, `usb_keyboard`
and `usbdev` tests run in the Pi 3 CI boot.

### Boot files

`boot/rpi3/build.sh`, which `cargo run -- rpi3` runs, builds the boot
files into `target/rpi3`:

- `armstub8-myos.bin` (`boot/rpi3/armstub8.S`, myos's own). Every core
  starts in it at address 0 and leaves at EL1, the level at which Limine
  boots a core without VHE (Limine refuses EL2 there). Core 0 jumps to
  U-Boot. The others wait in the device tree's spin table
  (`cpu-release-addr`), where Limine starts them. The stub stays at EL2
  as a minimal PSCI for the kernel's power-off and reboot
  (`docs/power.md`). Both reset the board through the power manager's
  watchdog; power-off sets the reset status the firmware reads as
  "halt".
- `u-boot.bin`: U-Boot (`UBOOT_VERSION` in `boot/rpi3/versions.env`,
  `rpi_3_defconfig`), built with GCC's AArch64 cross compiler
  (`gcc-aarch64-linux-gnu`), because a clang build crashes at run time.
  It finds `EFI/BOOT/BOOTAA64.EFI` on the first disk and starts it with
  the firmware's device tree.
- `bcm2710-rpi-3-b.dtb`: the Pi 3's device tree from the firmware
  release `FIRMWARE_TAG`. QEMU has no GPU firmware, so the launcher does
  its part: it fills the memory node in a copy (`qemu.dtb`), writes the
  tree's and U-Boot's addresses into a copy of the stub
  (`armstub8-qemu.bin`), and loads the three where the firmware would.
- `myos.img`: the same disk as `target/rpi4.img`, padded to a power of
  two for QEMU's SD card.

### How the kernel finds the Pi 3

The kernel finds the board in the device tree, like the Pi 4's. Its
interrupt controllers are Broadcom's, not a GIC
(`kernel/src/arch/aarch64/bcm2836.rs`). There are two, one in front of
the other:

- the per-core local controller (`brcm,bcm2836-l1-intc`): the timers,
  the mailboxes the IPIs go through, and the peripherals' interrupt,
  routed to core 0;
- behind it, the BCM2835's controller (`brcm,bcm2836-armctrl-ic`) for
  the peripherals. Its `bank, number` specifiers are numbered 32 + n for
  GPU interrupt n and 96 + n for basic interrupt n.

The kernel's console is the UART `stdout-path` names, the mini UART
(`brcm,bcm2835-aux-uart`).

### On a real Pi 3

Put these on a FAT32-formatted SD card:

- `bootcode.bin`, `start.elf`, `fixup.dat` and `bcm2710-rpi-3-b.dtb`
  from the
  [firmware release](https://github.com/raspberrypi/firmware/tree/master/boot)
  (`FIRMWARE_TAG`);
- `target/rpi3/armstub8-myos.bin` and `target/rpi3/u-boot.bin`;
- a `config.txt`:

```
arm_64bit=1
enable_uart=1
kernel=u-boot.bin
armstub=armstub8-myos.bin
```

The myos disk goes on a USB stick
(`sudo dd if=target/rpi3/myos.img of=/dev/sdX bs=4M conv=fsync`). U-Boot
boots it when the SD card has no `EFI/BOOT` of its own. myos itself then
runs from the initramfs. It sees the SD card (`/dev/mmcblk0`) and,
through the DWC2 driver (above), the USB stick, whose ESP and data
partition it mounts as `/boot` and `/data` as on the other boards. That
is what the QEMU boot shows; it is untried on a real Pi 3.

The console is the mini UART on GPIO 14 and 15 (the same pins as the
Pi 4's, above), at 115200 8N1.
