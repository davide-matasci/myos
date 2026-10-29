# USB

Eleven modules give myos a USB bus. The `usb` core owns the devices and
publishes the bus. Five controller drivers bring up the host controllers
and register them with the core. Five class drivers, plus `usbdev`, take
the devices:

- `xhci` drives PCI xHCI controllers.
- `ehci` drives USB 2.0's EHCI, and `uhci` and `ohci` its USB 1.1
  companions (UHCI and OHCI).
- `dwc2` drives the Raspberry Pi 3's controller (`docs/raspberry-pi.md`).
- `usb_hub` makes the devices behind hubs reachable.
- `usb_hid` feeds mice and tablets to the console's pointer
  (`/dev/console/mouse`, `docs/tty.md`), and keyboards to its keyboard
  input.
- `usb_audio` plays through USB speakers, as a card of the sound core
  (`docs/sound.md`).
- `usb_storage` turns memory sticks and disk enclosures into `/dev/sdX/`:
  `data` is the whole disk, `p<N>` a GPT partition, and `info` gives the
  disk's size, its `/proc/usb` device and vendor:product, and the vendor
  and model from its INQUIRY answer.
- `usbdev` hands whatever none of them takes to programs, as `/dev/usb`.
  A USB Wi-Fi adapter's driver runs this way, in the wifi daemon
  (`docs/wifi.md`).

A device plugged in after boot is enumerated. A device pulled out is gone
from `/dev`. `/proc/usb` lists what is there. Each module is in
`modules/<name>`.

## How the modules fit

Modules call only the `KernelApi`, never each other. They find each other
through its **service registry** (ABI 21): `service_register(name,
table)` publishes a `#[repr(C)]` function table, and
`service_lookup(name)` finds it. The tables are in `modules/abi/src/lib.rs`.

- The core publishes `UsbHostOps` as `usb`. A class driver looks it up in
  its `module_init` and registers a `UsbDriverOps` (`probe`,
  `disconnect`) with `driver_register`.
- The core publishes `UsbCoreOps` as `usbcore`. A controller module
  brings its controllers up and registers each with `controller_register`,
  handing the core a `UsbHcdOps` table: its root ports, the per-device
  state it keeps (a slot or an address record), and the transfers.

A lookup counts as a registration, so a class driver stays loaded while
the core may call it. `rmmod` refuses the core, because of its services
and its thread.

Adding a controller type means writing a module that implements
`UsbHcdOps`. The core, the class drivers and `/proc/usb` are the same
above every controller, and controllers of several types run side by
side.

### The core's thread

The core enumerates on **its own kernel thread** (`thread_spawn`, ABI
21), because resets, descriptor reads and `SET_CONFIGURATION` take
milliseconds and wait on completions, which an interrupt handler cannot
do. Every driver hook (`probe`, `disconnect`, a transfer's completion
callback) runs on that thread too, so a hub's `probe` can enumerate its
children synchronously.

A controller's interrupt handler only flags and wakes the thread. A
controller without an interrupt still works: the thread then runs every
20 ms and waiters poll.

A class driver may also start transfers from another context, such as a
`blk_read` from a user task. A device's entry is behind a `SleepLock`,
held across each controller call. The core never holds a device lock
across a driver hook, because hooks call back into it. When a device is
pulled out, the controller fails its transfers in flight (`USB_EGONE`)
first, so the lock comes free at once.

## Enumeration

A root port's connect event, or a hub's status report, wakes the thread.
It debounces, resets the port and walks the device through the usual
steps: an address, the device descriptor, the first configuration, the
endpoints configured at the controller, `SET_CONFIGURATION`. The core
then offers each interface to the class drivers in registration order
until one takes it. A driver registered later is offered the unclaimed
interfaces of every device.

A device's path is its route string (the port at every hub tier) and,
for a low- or full-speed device behind a high-speed hub, that hub's
transaction translator.

What the class drivers do with an interface:

- **Hubs** (class 9): power and reset the ports, then `hub_attach` and
  `hub_detach` the children as the status-change endpoint reports them.
  A detach tears down the nested children first.
- **Pointers** (class 3 with an X and a Y axis in its report
  descriptor): relative axes for a mouse, absolute ones for a tablet
  (scaled to 0–32767), the vertical wheel and buttons 1 to 5. A boot
  mouse whose descriptor does not parse falls back to the boot protocol.
- **Keyboards** (class 3, subclass 1, protocol 1): always the boot
  protocol and its fixed report. Changes go to the console's `keyboard`
  service in Linux `KEY_*` codes (`modules/usb_hid/src/keyboard.rs` maps
  the usages), the same path as the console's own keyboard. The console
  repeats held keys and sets the Caps Lock LED.
- **Mass storage** (class 8, subclass 6, protocol `0x50`, bulk-only
  SCSI): `blk_register` as `sd?` (the kernel hands out the letter, shared
  with the virtio-scsi disks).

### Unplugging a disk

The block device is unregistered when nothing holds it. While a
filesystem is mounted from it or an fd is open on it, the kernel refuses
(`MYOS_EBUSY`). The entry then stays in `/dev`, failing its I/O, until
the disk is unmounted and the fds are closed. `/proc/usb` marks such a
device `gone`.

## Isochronous transfers and USB audio

A class driver selects an interface's alternate setting with
`set_interface`; the controller drops and adds the endpoints. An
isochronous OUT endpoint of the active setting takes packets with
`isoch_queue`, one per service interval, up to 64 in flight
(`USB_ISOCH_MAX`). The core's thread reports completed packets to the
driver's callback. A missed interval counts as completed, and its packet
is lost.

`usb_audio` takes UAC1 and UAC2 devices. It picks the streaming
interface's alternate setting with an isochronous OUT endpoint and a
type I PCM format in 16, 24 or 32-bit slots, preferring 16-bit. It
registers a card `usb-audio VVVV:PPPP` with one playback stream and keeps
about 24 ms of packets queued, filled from the sound core's buffer.
Stopping the stream returns to alternate setting 0, which frees the bus's
bandwidth.

`usb_audio` does not read an asynchronous endpoint's explicit feedback,
so its packets carry the nominal rate. No QEMU device speaks UAC2 or
plays at high speed, so those paths are untested.

## EHCI and its companions

A PC's USB 2.0 port is usually two controllers: an EHCI for high-speed
devices, and companions for full- and low-speed ones (UHCI on Intel's and
VIA's chipsets, OHCI on AMD's and most others and on many SoCs).

`ehci` routes every port to itself, then hands a full- or low-speed
device back to the companion (the port owner bit). `ehci` is loaded
before `uhci` and `ohci`, so the routing is settled when the companions
look. A full- or low-speed device behind a high-speed hub stays on the
EHCI, through the hub's transaction translator.

Before taking a controller, `ehci` and `ohci` ask the firmware for it,
and `uhci` turns off the firmware's legacy keyboard emulation.

The three drivers keep a queue head (or OHCI endpoint descriptor) per
endpoint and put isochronous OUT packets in the frame list. Their
structures stay below 4 GiB (32-bit pointers). UHCI's registers are in
PCI I/O space (`pci_io_map`, `docs/pci.md`), and its port changes raise
no interrupt, so its thread polls them four times a second. QEMU's EHCI
skips split isochronous descriptors and QEMU has no high-speed
isochronous device, so EHCI's isochronous path does not run in the test
boots.

## `/proc/usb`

```
controller 0 ports 8 irq on events 86 irqs 83
0x001 parent 0x000 port 5 full 0409:55aa class 09 if0=09/00/00:usb_hub hub 8
0x002 parent 0x001 port 1 full 46f4:0001 class 00 if0=08/06/50:usb_storage sdb
0x003 parent 0x000 port 7 high 0627:0001 class 00 if0=03/01/02:usb_hid mouse
0x004 parent 0x000 port 2 super 46f4:0001 class 00 if0=08/06/50:usb_storage sdc
```

A `controller` line gives its root ports, whether its interrupt is on,
and the events and interrupts seen. Each device line gives:

- the id (the controller in the high byte);
- the parent hub (0: a root port) and the port;
- the speed and vendor:product;
- the device class;
- each interface's class/subclass/protocol, the driver that took it, and
  what the driver made of it: a disk's name in `/dev`, `mouse` or
  `tablet`, a hub's port count.

The port path (parent and port, up to the root) says where a disk is
plugged in.

## `/dev/usb`

`usbdev` registers its driver last, so it gets the interfaces the
kernel's drivers passed on. A device gets a directory named after its
`/proc/usb` id, such as `/dev/usb/002/`:

- `info`: `id VVVV:PPPP`, `class CC:SS:PP`, `speed low|full|high|super`,
  a line per interface taken (`if0 ff:ff:ff`) and per bulk or interrupt
  endpoint (`ep81 bulk in 64`: address, type, direction, max packet
  size), then `held` while a program drives the device.
- `ctl`: held by one program at a time, root only, from its `open` to its
  last close. It reads as `info`. A write of `clear 81` recovers a stalled
  endpoint. The files below are there only while `ctl` is held.
- `ep0`: control transfers. A write is one transfer: the 8-byte setup
  packet (little endian), then an OUT transfer's data. An IN transfer's
  data is the next read.
- `epXX`, one per bulk or interrupt endpoint. An OUT write is one
  transfer, of up to a second. An IN endpoint keeps a transfer queued
  while the device is held; a read returns one completed transfer whole,
  and `poll` says one is there. Four completed transfers wait; more are
  dropped.

When the device is pulled out, every file's I/O fails (a read returns end
of file). The directory goes when `ctl` is let go.

Unlike `pcidev`, the module is loaded at boot. A USB device reaches only
the buffers the controller gives it, never the program's memory, so
`/dev/usb` needs no IOMMU. `user/wifi/src/usb.rs` is a driver of this
kind.

## Testing

Every boot carries three USB buses (`src/main.rs`,
`src/usb2.rs`):

- a `qemu-xhci` with a hub, a stick behind it, a second stick plugged and
  unplugged by a test, a mouse (a tablet in a boot in a window), a
  keyboard plugged in by a test, a speaker, and in a test boot an FTDI
  serial adapter whose line loops back through the host;
- an ICH9 EHCI with its UHCI companions: an MTP device, and a hub with a
  speaker behind it;
- a second EHCI with OHCI companions: a hub with a speaker, and a
  smartcard reader.

The tests are in `user/tests/kernel.sh` (`usb_disk`, `usb_hotplug`,
`usbdev`, `usb_ehci`, `usb_uhci`, `usb_ohci`), `user/c/test.sh` (`mouse`,
`usb_keyboard`) and `user/sndd/test.sh` (`sound_usb`, where the launcher
finds the tone in each speaker's WAV file). The host plugs devices in
through the QEMU monitor (`user/tests/host.sh`, `user/c/host.sh`).

## Limits

- Controllers: twelve in all; two xHCI, four EHCI, eight UHCI and eight
  OHCI ones at most. 32 devices per controller.
- Hubs five deep, 8 hubs, 8 disks, and 8 HID devices (pointers and
  keyboards together). Four interrupt transfers queued per device.
- Controller types: xHCI, EHCI, UHCI and OHCI on PCI, and the Raspberry
  Pi 3's DWC2 from the device tree. A SoC's `snps,dwc3` would be another
  controller module.
- The DWC2 takes 8 transfers at once (its channels), and a queued
  transfer holds a channel. Its split transactions are untested, because
  QEMU has no high-speed hub.
- Pointers: no horizontal wheel, no buttons past the fifth, one pointer
  per interface.
- Keyboards: the boot protocol only, so six keys at once. No media or
  system keys (another interface, in the report protocol). Only the Caps
  Lock LED, since the console has no Num or Scroll Lock state.
- Mass storage: 512-byte sectors only.
- Isochronous transfers: OUT only, so no capture (issue #610); not on the
  DWC2 (issue #608). Two endpoints per device on the xHCI; eight in all
  on an EHCI, UHCI or OHCI. 4 USB speakers.
- No streams. No MSI for the devices: on aarch64 the controller's own
  interrupt is INTx (`docs/interrupts.md`).
- Bulk transfers go through a 64 KiB bounce buffer per controller, one at
  a time; a queued bulk IN endpoint has a page of its own. A control
  transfer's data stage is 1 KiB at most.
- `/dev/usb`: 8 devices, 4 interfaces and 16 endpoints each. An IN
  transfer is a page (bulk) or up to 64 bytes (interrupt). No isochronous
  endpoints. One program per device.
- Real hardware: the BIOS handoff (`USBLEGSUP`) is done; the EHCI-to-xHCI
  port routing of older Intel chipsets is not. The Raspberry Pi 4's VL805
  is taken with the firmware its UEFI firmware loaded
  (`docs/raspberry-pi.md`). A controller whose DMA is not coherent gets
  the cache maintenance of its handle (`docs/pci.md` "DMA").
