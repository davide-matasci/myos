# USB

Eleven modules give myos a USB bus. The `usb` core owns the devices and
publishes the bus. Five controller drivers bring up the host controllers
and register them with the core. Five class drivers, plus `usbdev`, take
the devices:

- `xhci` drives PCI xHCI controllers.
- `ehci` drives USB 2.0's EHCI, and `uhci` and `ohci` its USB 1.1
  companions (UHCI and OHCI).
- `dwc2` drives the Raspberry Pi 3's controller.
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
from `/dev`. `/proc/usb` lists what is there.

Every CI boot carries three USB buses ("Testing" below has the details):

- an xHCI with a hub, a mouse, two sticks (one plugged in by a test), a
  serial adapter and a speaker; a test also plugs in a keyboard;
- an ICH9 EHCI with its UHCI companions, an MTP device, and a hub with a
  speaker behind it;
- a second EHCI with OHCI companions, a hub with a speaker behind it, and
  a smartcard reader.

## Modules

| Module | What |
|--------|------|
| `modules/usb` | The core: the device table, enumeration (descriptors, configuration), the class drivers and the interfaces offered to them, hubs' children and their route strings, root-port hot-plug on its own kernel thread, the `UsbHostOps` service, `/proc/usb`; the host controllers' `UsbCoreOps` service |
| `modules/xhci` | The xHCI controller driver (PCI class `0C/03`, interface `30`): rings, interrupter, device slots and contexts, the transfers; registered with the core as a `UsbHcdOps` |
| `modules/ehci` | The EHCI controller driver (PCI `0C/03`, interface `20`): the asynchronous schedule (a QH per control and bulk endpoint) and the periodic one (the interrupt endpoints' QHs), qTD chains, split transaction fields for devices behind a high-speed hub, isochronous OUT packets in iTDs (high speed) and split siTDs (full speed behind a hub); hands full- and low-speed devices on its ports to its companion controllers |
| `modules/uhci` | The UHCI controller driver (PCI `0C/03`, interface `00`; registers in I/O space): a frame list and a QH per endpoint, a TD per packet, isochronous OUT packets in the frame list; an EHCI's companion or a controller of its own |
| `modules/ohci` | The OHCI controller driver (PCI `0C/03`, interface `10`; registers in memory, BAR 0): the control, bulk and periodic lists of EDs (one per endpoint), general TDs, isochronous TDs of up to eight packets; an EHCI's companion or a controller of its own |
| `modules/dwc2` | The DWC2 controller driver (the Raspberry Pi 3's, from the device tree): host channels, the port, split transactions; the same `UsbHcdOps` (`docs/raspberry-pi.md`) |
| `modules/usb_hub` | Hub class (9): hub descriptor, port power and reset, the status-change interrupt endpoint; children through `hub_attach` / `hub_detach` |
| `modules/usb_storage` | Mass storage class (8/6/0x50, bulk-only SCSI): `blk_register` as `sd?` (the kernel hands out the letter, shared with the virtio-scsi disks); `blk_unregister` on unplug |
| `modules/usb_hid` | HID class (3) pointers: the report descriptor's X and Y axes, wheel and buttons (or the boot mouse report), its interrupt endpoint's reports into the console's `pointer` service; boot keyboards (3/1/1): the boot report's keys into the console's `keyboard` service, its Caps Lock LED from the console |
| `modules/usb_audio` | Audio class (1) playback, UAC1 and UAC2: the streaming interface's alternate setting with an isochronous OUT endpoint and PCM in 16, 24 or 32-bit slots, a playback stream of the sound core fed one packet a service interval |
| `modules/usbdev` | Every interface the others leave: `/dev/usb/<id>/`, its control, bulk and interrupt transfers as files for a driver that runs as a program |

## How the modules reach each other

### Services

Modules call only the `KernelApi`, never each other. They find each other
through its **service registry** (added in ABI 21):
`service_register(name, table)` publishes a `#[repr(C)]` function table,
and `service_lookup(name)` finds it.

- The core publishes [`UsbHostOps`](../modules/abi/src/lib.rs) as `usb`.
  A class driver looks it up in its `module_init` and registers a
  `UsbDriverOps` (`probe`, `disconnect`) with `driver_register`.
- The core also publishes `UsbCoreOps` as `usbcore`, for the **host
  controller drivers**. A controller module (`xhci`, `ehci`, `uhci`,
  `ohci`, or `dwc2` for the Raspberry Pi 3) brings its controllers up and
  registers each with `controller_register`.

A lookup counts as a registration, so a class driver stays loaded while
the core may call it. The core stays loaded because of its services and
its thread: `rmmod` refuses both.

`controller_register` hands the core a `UsbHcdOps` table, the
controller's part of the bus:

- its root ports: `ports`, `port_status`, `port_reset`;
- the per-device state the controller keeps: `device_new` (a slot or an
  address record), `device_address`, `device_ep0`, `device_configure`,
  `device_hub`, `device_abort`, `device_free`;
- the transfers: `control`, `bulk`, `transfer_start` and
  `transfer_take`, `clear_halt`.

The core calls a controller's table one call per device at a time,
holding the device's lock across the call. The controller's interrupt
handler calls the core's `port_change` and `wake`, which only flag and
wake the thread. A controller without an interrupt says so from `poll`,
which the thread calls on every pass; the thread then runs every 20 ms.

Adding a controller type means writing a module that implements
`UsbHcdOps`. The core, the class drivers and `/proc/usb` are the same
above every controller, and controllers of several types run side by
side.

### The core's thread

The core enumerates on **its own kernel thread** (`thread_spawn`, also
from ABI 21). Resets, descriptor reads and `SET_CONFIGURATION` take
milliseconds and wait on completions, which an interrupt handler cannot
do. Every driver hook (`probe`, `disconnect`, the completion callback of
`transfer_start`) runs on that thread too. So a hub's `probe` may reset
its ports and enumerate its children synchronously through `hub_attach`.

A class driver may also start transfers from another context, such as a
`blk_read` from a user task. Those run in that context. A device's table
entry is behind a `myos_abi::SleepLock`, which the transfer holds until
the controller driver returns. The thread holds the same lock while it
enumerates the device.

The core holds no device lock across a driver hook, because `probe`,
`disconnect` and a completion callback call back into it.

When a device is pulled out:

1. The controller fails its transfers in flight (`device_abort`,
   `USB_EGONE`) before `detach` waits for the device's lock, so the lock
   comes free at once.
2. A queued transfer gets the error in its callback (`transfer_take`
   hands it over).
3. The controller's records are reset with the device (`device_free`), so
   the next device in the entry starts clean.

### Lock order

The locks nest in one order:

1. a class driver's own slot lock (a disk's);
2. the core's device entry;
3. the controller's locks (in `xhci`: its device entry, then its command
   ring or bounce buffer).

The core never holds a device lock across a driver hook, so the reverse
order never happens. The interrupt handler holds none of these locks.

### xHCI

The `xhci` driver reaches its controller as `&Controller` from every
context:

- The command ring and the bulk bounce buffer are each behind a
  `SleepLock`.
- The completions are atomics outside the device's lock. The interrupt
  handler fills them and `wake`s the waiter (`wake` is the third ABI 21
  entry).
- The event ring belongs to whoever holds its busy flag: the handler, or
  a waiter polling it.
- The rest is atomics or written by one context alone.

Whoever finds the event ring busy still acknowledges the interrupter
(`IMAN.IP`) and asks the holder to look again once it lets go
(`hc::process_events`). On aarch64 and riscv64 the interrupt is a level
INTx line: unacknowledged, it would fire again at once and, on the CPU
whose waiter it interrupted, never let that waiter finish.

Without an interrupt (`pci_irq_enable` failed) the bus still works: a
waiter polls the event ring every 2 ms, and the core's thread runs every
20 ms.

## Enumeration

A root port's connect event, or the initial scan, wakes the thread. It
debounces for 100 ms, resets a USB 2 port (a USB 3 link trains itself),
reads the port speed, and runs the enumeration every new device goes
through:

1. `device_new` with the device's path:
   - the route string (the port at every hub tier, 4 bits each);
   - the speed and the root port;
   - the transaction translator: for a low- or full-speed device behind a
     high-speed hub, that hub and port;
   - the address the core gives it.

   On xHCI this is `Enable Slot`, the slot's output device context into
   the DCBAA, and endpoint 0's ring at the speed's default max packet
   size.
2. `device_address`. On xHCI this is `Address Device` with the input
   context; a controller without that command sends `SET_ADDRESS`.
3. The device descriptor, 8 bytes first: a full-speed device may use
   another max packet size for endpoint 0 (`device_ep0`; xHCI:
   `Evaluate Context`).
4. The first configuration descriptor: its interfaces (alternate setting
   0) and their endpoints. Isochronous endpoints wait for their alternate
   setting (below). The core keeps the descriptor for `set_interface`.
5. `device_configure` with every endpoint (xHCI: `Configure Endpoint`,
   one transfer ring each), then `SET_CONFIGURATION`.
6. The core offers every interface to the class drivers in registration
   order until one takes it. A driver registered later is offered the
   unclaimed interfaces of every device.

### Hubs

A hub's `probe`:

1. reads the hub descriptor;
2. tells the core the port count (`hub_configure`; on xHCI, the slot
   context's hub fields);
3. powers the ports;
4. resets every connected port and calls `hub_attach(hub, port, speed)`,
   which runs the same six steps for the hub's child, one tier deeper in
   the route string.

The hub's status-change endpoint then reports connects and disconnects.
`hub_detach(hub, port)` tears a child down:

1. its own children first (a nested hub);
2. its waiters, failed with `USB_EGONE` (a callback transfer's callback
   runs with the error);
3. its drivers' `disconnect`;
4. `device_free` (xHCI: `Disable Slot`, its memory back to the module's
   pool).

### Pointers

`usb_hid` claims a class 3 interface whose HID report descriptor
(`GET_DESCRIPTOR` of the interface, type `0x22`) has an X and a Y axis of
the generic desktop page. A mouse's axes are relative. A tablet's are
absolute (QEMU's `usb-tablet`), and `usb_hid` scales them to the
pointer's 0–32767.

It reads the report that carries those axes (by its report id, when the
device numbers its reports), the vertical wheel, and buttons 1 to 5:
left, right, middle, side, extra. A boot mouse whose descriptor does not
parse is switched to the boot protocol (`SET_PROTOCOL`) and its fixed
three-byte report.

`usb_hid` asks for reports only on change (`SET_IDLE` 0). It keeps one
transfer queued on the interrupt IN endpoint (`transfer_start`) and
queues the next from the completion, on the host's thread. Motion goes to
the console as it comes (for a tablet, its position when it moved).
Buttons go as presses and releases of what changed.

### Keyboards

`usb_hid` switches a boot keyboard's interface (class 3, subclass 1,
protocol 1) to the boot protocol, whatever its descriptor says. Every
keyboard has that protocol, and its eight-byte report is fixed: the
modifier bits, then up to six keys held, as usages of the Keyboard/Keypad
page.

What changed from the last report goes to the console's `keyboard`
service as presses and releases in Linux `KEY_*` codes, the same path as
the console's own keyboard (`docs/tty.md`).
`modules/usb_hid/src/keyboard.rs` maps the usages. A report of
ErrorRollOver (more keys held than the report can name) changes nothing.

- A USB keyboard does not repeat a held key; the console does.
- After each report, and when the keyboard is plugged in, `usb_hid` asks
  the console which lock LEDs to light (Caps Lock). It sends the keyboard
  an output report (`SET_REPORT`) when that changed.
- A keyboard pulled out releases the keys it held.

### Mass storage

`usb_storage` claims an interface of class 8, subclass 6, protocol `0x50`
with a bulk IN and a bulk OUT endpoint. It then:

1. runs `TEST UNIT READY` until the unit is ready (sticks need a moment);
2. runs `READ CAPACITY(10)` and refuses a sector size other than 512
   bytes;
3. registers the disk.

`READ(10)` and `WRITE(10)` move each request as one command: CBW out,
data, CSW in. On errors:

- a stalled endpoint is cleared (`clear_halt`);
- a failed command gets a `REQUEST SENSE` and one retry;
- a phase error gets a bulk-only reset.

## Alternate settings and isochronous transfers

A class driver selects an interface's alternate setting with
`set_interface`. The core finds the endpoints of the old and the new
setting in the kept configuration descriptor, has the controller drop
and add them (`endpoints_change`; xHCI: `Configure Endpoint` with the
input context's drop and add flags), and sends `SET_INTERFACE`.

An isochronous OUT endpoint of the active setting takes packets with
`isoch_queue`:

- up to 64 packets in flight (`USB_ISOCH_MAX`; past that, `USB_EBUSY`);
- each up to 1024 bytes;
- one per service interval, from the next free one.

`isoch_start` registers the driver's callback. The core's thread calls it
with the number of packets completed since the last call (the
controller's `isoch_reap`), until `isoch_stop` or until the device goes
(`USB_EGONE`).

On the xHCI, the driver copies the packets into four pages of slots per
endpoint and queues one Isoch TRB each (Start Isoch ASAP, an interrupt on
completion). A missed interval counts as completed, and its packet is
lost. An isochronous endpoint's context has no error count. Its interval
is the full-speed frame count's exponent (`bInterval - 1`), plus 3 in
microframes.

The DWC2 drives no isochronous transfers yet (issue #608). UHCI's, OHCI's
and EHCI's are described under "EHCI and its companions".

### USB audio

`usb_audio` claims the audio control interface (class 1, subclass 1:
nothing to drive) and the streaming one (subclass 2). It finds the
streaming interface's alternate setting with an isochronous OUT endpoint
and a type I PCM format in 16, 24 or 32-bit slots, preferring a 16-bit
one. It registers a card `usb-audio VVVV:PPPP` with one playback stream:
the format's channels, and those of its sampling frequencies the sound
core knows.

- A UAC1 interface (protocol 0) lists the frequencies in its format.
- A UAC2 interface (protocol 0x20: high-speed DACs and interfaces, and
  full-speed ones) names the channels in its general descriptor and in a
  terminal of the control interface. That terminal's clock source answers
  a RANGE request with the frequencies, through a clock selector's
  current input or a multiplier's source.

Starting the stream:

1. selects the alternate setting;
2. sets the sampling frequency (UAC1: the endpoint's `SET_CUR`, which a
   speaker with one rate may refuse; UAC2: the clock source's CUR);
3. keeps about 24 ms of packets queued, a third at a time, one per
   service interval of the endpoint:
   - at full speed, a millisecond (`bInterval` 1: 24 packets, 8 at a
     time);
   - at high speed, 2^(`bInterval` - 1) microframes, 48 packets at most
     as `USB_ISOCH_MAX` allows (6 ms at every microframe).

The packets come from the sound core's buffer, with silence for what it
lacks. 44.1 kHz carries 44 or 45 frames a millisecond. The core's 16-bit
samples go in the top bytes of a wider slot. Each completion queues more.

Stopping the stream goes back to setting 0, which gives the bus its
bandwidth back.

`usb_audio` does not read an asynchronous endpoint's explicit feedback
(an isochronous IN endpoint), so its packets carry the nominal rate.
Capture (a microphone's isochronous IN) is issue #610. No QEMU device
speaks UAC2 or plays at high speed, so those paths are untested.

## EHCI and its companions

A USB 2.0 port of a PC is usually two controllers: an EHCI for its
high-speed devices, and companions for the full- and low-speed ones. The
companions are UHCI on Intel's and VIA's chipsets, and OHCI on AMD's,
NVIDIA's, SiS's and ALi's, and on many SoCs.

`ehci` sets the EHCI's CONFIGFLAG, which routes every port to it, and
looks at what is attached:

- A low-speed device (the line in the K state) goes back to the companion
  at once (the port owner bit).
- A full-speed device goes back after its reset, which leaves the port
  disabled. The reset fails, the core lets the port be, and the companion
  sees the device arrive.

`ehci` is loaded before `uhci` and `ohci`, so the routing is settled when
the companions look.

A full- or low-speed device behind a high-speed hub stays on the EHCI,
reached through the hub's transaction translator. Its QHs name the hub
and port; an interrupt endpoint's QH also has its start- and
complete-split masks.

Before taking the controller, `ehci` asks the firmware for it (the
USBLEGSUP capability). Likewise `uhci` turns off the firmware's legacy
keyboard emulation (LEGSUP), routing its interrupt to the PIRQ line.

### EHCI and UHCI schedules

Both keep a QH per endpoint, set up with the configuration and kept while
the device is there. A transfer's descriptors go on the endpoint's idle
QH:

- EHCI qTDs, up to five pages each;
- UHCI TDs, a packet each, each with its data toggle.

The waiter looks at them when the controller interrupts, or a
millisecond at a time without an interrupt.

- EHCI keeps a bulk or interrupt endpoint's data toggle in its QH; UHCI
  keeps it in the module.
- An EHCI QH goes away with the async advance doorbell; a UHCI QH two
  frames after it is unlinked.
- EHCI interrupt endpoints, and UHCI's (on its interrupt skeleton), are
  visited every frame, whatever their interval.
- UHCI's port changes raise no interrupt. The module's thread looks at
  them four times a second, so an idle CPU still sleeps.
- Their structures are below 4 GiB (32-bit pointers,
  `DmaDevice::below`).
- UHCI's registers are in PCI I/O space (`KernelApi::pci_io_map`,
  `docs/pci.md`).

### Isochronous OUT on UHCI and EHCI

A UHCI isochronous OUT packet is a TD in the frame list entry of the
frame it goes out in. Packets are queued from two frames ahead of the
controller (`FRNUM`) on, one a frame. Once its frame has passed, a TD is
taken out and counted (`isoch_reap`).

An EHCI's isochronous OUT packets go in the frame list too, in front of
the anchor QH. Their descriptors come from a pool of the controller's
(8 pages of 128-byte slots, made with its first isochronous endpoint):

- A high-speed endpoint's packets go in iTDs, one a frame, with a packet
  in each microframe of its service interval (up to eight). `Mult`, from
  `wMaxPacketSize` bits 12:11, lets a packet take up to three
  transactions.
- A full-speed endpoint behind a high-speed hub's transaction translator
  uses split siTDs, one a packet. Its start-splits are 188 bytes each, in
  the frame's first microframes, and an OUT needs no complete-split.

The descriptors go in from the controller's isochronous scheduling
threshold ahead (HCCPARAMS) on, and come out once inactive or once their
frame has passed. A full-speed speaker on an EHCI's root port plays
through the companion. QEMU's EHCI skips siTDs, and QEMU has no
high-speed isochronous device, so neither path runs in the test boots.

### OHCI

OHCI's registers are memory-mapped (BAR 0). Its schedule is lists of
endpoint descriptors (EDs), each with a queue of transfer descriptors
(TDs):

- Control and bulk EDs are in two lists of their own.
- Interrupt and isochronous EDs are reached from every entry of the
  interrupt table in the HCCA (the block the controller shares with the
  module), so an interrupt endpoint is asked every frame too.
- Every list starts at a skipped anchor ED, after which an endpoint's ED
  is linked in.

An ED's queue ends in a dummy TD. A transfer fills the dummy and the
slots after it (a ring in the ED's page) and moves the tail to a new
dummy. The controller retires the TDs, moving the head along, and halts
the ED on an error.

- A bulk or interrupt endpoint's data toggle is kept in its ED.
- A bulk transfer is one TD, up to two pages of the bounce buffer.
- A short IN packet ends a TD without an error.
- An ED leaves a periodic list a frame before its page is reused. It
  leaves the control or bulk list with that list stopped for a frame,
  after the controller's current ED is moved past it.

Isochronous OUT packets go in isochronous TDs of up to eight frames on
the endpoint's ED, their packets in a region of a page. They are queued
from two frames ahead of the controller (`HcFmNumber`). A TD is counted
once the head has passed it, sent or expired.

Before taking the controller, `ohci` asks SMM for it (the
OwnershipChangeRequest, while InterruptRouting is set), or resumes what a
BIOS driver left suspended. A root port change raises its interrupt, so
`ohci` needs no thread of its own.

## Hot-plug and unplug

Plugging in: a root port's change event or a hub's status report, then
the enumeration above.

Unplugging: the block device is unregistered (`blk_unregister`) when
nothing holds it. While a filesystem is mounted from it or an fd is open
on it, the kernel refuses (`MYOS_EBUSY`). The entry then stays in `/dev`,
failing its I/O, until the disk is unmounted (`umount`) and the fds are
closed. `/proc/usb` marks such a device `gone`.

## `/proc/usb`

```
controller 0 ports 8 irq on events 86 irqs 83
0x001 parent 0x000 port 5 full 0409:55aa class 09 if0=09/00/00:usb_hub hub 8
0x002 parent 0x001 port 1 full 46f4:0001 class 00 if0=08/06/50:usb_storage sdb
0x003 parent 0x000 port 7 high 0627:0001 class 00 if0=03/01/02:usb_hid mouse
0x004 parent 0x000 port 2 super 46f4:0001 class 00 if0=08/06/50:usb_storage sdc
```

A `controller` line gives its root ports, then what its driver says:
whether its interrupt is on, and the events and interrupts seen.

Each device line then gives:

- the id (the controller in the high byte);
- the parent hub (0: a root port) and the port;
- the speed and vendor:product;
- the device class;
- each interface's class/subclass/protocol, with the driver that took it
  and what the driver made of it. For a disk this is its name in `/dev`,
  the label `usb_storage` hands the core through `interface_label` once
  the disk is registered. For a pointer it is `mouse` or `tablet`; for a
  hub, its port count.

The port path (parent and port, up to the root) says where a disk is
plugged in.

## `/dev/usb`

`usbdev` registers its driver last (it is loaded after `usb_hid`), so
the host offers it the interfaces the kernel's drivers passed on, and it
takes them all. A vendor-class device (a Wi-Fi adapter, a serial adapter)
gets a directory named after its `/proc/usb` id, such as `/dev/usb/002/`:

- `info`: `id VVVV:PPPP`, `class CC:SS:PP`, `speed low|full|high|super`,
  a line per interface taken (`if0 ff:ff:ff`) and per bulk or interrupt
  endpoint (`ep81 bulk in 64`: the address in hex, the type, the
  direction, the max packet size), then `held` while a program drives
  the device.
- `ctl`: one program holds it at a time, root only, from its `open` to
  its last close; a second `open` fails. It reads as `info`. A write of
  `clear 81` recovers a stalled endpoint (`CLEAR_FEATURE(ENDPOINT_HALT)`).
  The files below are there only while `ctl` is held.
- `ep0`: control transfers. A write is one transfer: the 8-byte setup
  packet (`bmRequestType`, `bRequest`, `wValue`, `wIndex`, `wLength`,
  little endian), then an OUT transfer's data. An IN transfer's data is
  the next read. A transfer that fails (a stall, a timeout) fails the
  write.
- `epXX`, one per bulk or interrupt endpoint:
  - An OUT endpoint's write is one transfer, of up to a second.
  - An IN endpoint's file, once opened, has a transfer queued
    (`transfer_start`, a page at most for bulk). The module keeps one
    queued while the device is held, so a `poll` before the first read
    sees what comes.
  - A read returns one completed transfer whole (a short one as it came)
    or waits for one. `poll` says one is there.
  - Four completed transfers wait to be read. More are dropped until the
    reader catches up.

When the device is pulled out, every file's I/O fails (a read returns
end of file) and a waiting reader wakes. The directory goes when `ctl` is
let go. Closing `ctl` drops what was not read; a transfer still queued
then completes into nothing.

Unlike `pcidev`, the module is loaded at boot. A USB device reaches only
the buffers the controller gives it, never the memory of the program that
drives it, so `/dev/usb` needs no IOMMU.

## Testing

The boot markers `[ OK ] usb`, `xhci`, `ehci`, `uhci`, `ohci`,
`usb_hub`, `usb_storage`, `usb_hid`, `usb_audio`, `usbdev` cover the
modules' init.

### The xHCI

The launcher gives every boot a `qemu-xhci` with five ports of each kind
(`p2=5,p3=5`). On it:

| Port | Device | Tests |
|------|--------|-------|
| `port=1` (a USB 2 port, number 6 of the controller's ten) | a `usb-hub`, full speed like every QEMU hub. Behind it: a `usb-storage` (`/dev/sdb/data`, the same FAT volume as `/dev/vda/data` in its own image file), and in a test boot a `usb-serial` on its second port | `usb_disk`, `usbdev` |
| `port=2` (a USB 3 port: a SuperSpeed device) | a second stick, plugged and unplugged by a test | `usb_hotplug` |
| `port=3` (number 8 of the controller's, a high-speed device) | a `usb-mouse` in a test boot; a `usb-tablet` in a boot in a window | `mouse` |
| `port=4` | a `usb-kbd`, plugged in by a test | `usb_keyboard` |
| `port=5` | a `usb-audio` speaker (full speed, 48 kHz stereo) | `sound_usb` |

- `kernel.sh` waits for `/dev/sdb/data`, checks `/proc/usb`, and mounts
  and reads the volume (`usb_disk`). It then plugs, reads and unplugs the
  second stick (`usb_hotplug`): `user/tests/host.sh usb-plug` attaches
  the drive through the QEMU monitor, and `usb-unplug` removes it.
- `user/c/test.sh`'s `mouse` test moves the mouse and clicks it through
  the monitor (`user/c/host.sh mouse`: `mouse_move`, `mouse_button`)
  while `mouse_smoke` reads `/dev/console/mouse`. `packages/x11-apps`
  checks the same through Xfbdev and xev.
- A boot in a window (`cargo run` without a test) has a `usb-tablet` on
  `port=3`, which follows the host's pointer without grabbing it. The
  tests use the mouse because the monitor's `mouse_move` is relative.
- `user/c/test.sh`'s `usb_keyboard` test has the host plug in the
  `usb-kbd` (`user/c/host.sh usb-kbd plug`). The keyboard then takes the
  monitor's `sendkey`, since QEMU types on the keyboard added last. The
  test checks `/dev/console/kbd`'s events, a line typed on tty9 with Caps
  Lock and a held key's repeats, that the console's own keyboard's
  interrupt count did not move, and the unplug.
- The `usb-serial` is QEMU's FTDI FT232 (vendor class). Its line is a UDP
  chardev sending to its own port on the host's loopback (5555), so what
  goes out on its bulk OUT endpoint comes back on its bulk IN one.
  `kernel.sh`'s `usbdev` test finds it in `/dev/usb`, holds its `ctl`,
  reads its device descriptor through `ep0`, and loops a string through
  `ep02` and `ep81` (after the two status bytes the FTDI puts first).
- In a test boot the speaker plays into a WAV file.
  `user/sndd/test.sh`'s `sound_usb` plays a tone through it, and the
  launcher finds the tone in the file (`docs/sound.md`).

### EHCI with UHCI companions

Every boot also has an ICH9 EHCI (`ich9-usb-ehci1`) with its three UHCI
companions (`ich9-usb-uhci1` to `3`, ports 1–2, 3–4, 5–6), as UTM's
"USB 2.0" setting makes them (`src/usb2.rs`). A test boot puts on it:

- on port 1, QEMU's MTP responder (`usb-mtp`), a high-speed device no
  kernel driver takes, so usbdev's;
- on port 3, a `usb-hub` with a `usb-audio` speaker behind it, both full
  speed, so the EHCI hands them to the second companion.

`kernel.sh`'s `usb_ehci` checks that the MTP device is on the EHCI at
high speed (512-byte bulk packets), reads its device descriptor through
`ep0`, and opens an MTP session over its bulk endpoints. `usb_uhci`
checks that the hub and the speaker are on a UHCI and that nothing
full-speed stayed on the EHCI. `sound_usb` plays through every speaker,
and the launcher finds the tone in the UHCI and OHCI speakers' WAV files
too.

### EHCI with OHCI companions

A second ICH9 EHCI (`ich9-usb-ehci2`) has two `pci-ohci` companions of
three ports each, as AMD's chipsets pair them. A test boot puts on it:

- on port 1 (the first OHCI's), the same hub and speaker;
- on port 4 (the second OHCI's), QEMU's smartcard reader (`usb-ccid`,
  usbdev's).

`usb_ohci` checks that all three are on an OHCI, reads the reader's
device descriptor through `ep0`, and has it answer a CCID GetSlotStatus
over its bulk endpoints.

## Limits

- Controllers: twelve in all; two xHCI, four EHCI, eight UHCI and eight
  OHCI ones at most. 32 devices per controller.
- Hubs five deep, 8 hubs, 8 disks, and 8 HID devices (pointers and
  keyboards together): the class modules' tables. Four interrupt
  transfers queued per device.
- Controller types: xHCI, EHCI, UHCI and OHCI on PCI, and the Raspberry
  Pi 3's DWC2 from the device tree. A SoC's `snps,dwc3` would be another
  controller module.
- The DWC2 takes 8 transfers at once (its channels). A queued transfer
  (an interrupt endpoint's report, a bulk IN endpoint's page) holds a
  channel for as long as it is queued. Its split transactions are
  untested, because QEMU has no high-speed hub.
- Pointers: the report descriptor is read up to 512 bytes and the reports
  up to 64. No horizontal wheel, no buttons past the fifth. One pointer
  per interface: the reports of a composite device's other collections
  (a keyboard's) are skipped.
- Keyboards: the boot protocol only, so six keys at once (more is
  ErrorRollOver). No media or system keys: they are another interface, in
  the report protocol, which `usb_hid` does not take. Only the Caps Lock
  LED, since the console has no Num or Scroll Lock state. No test checks
  the LED report, because QEMU shows it nowhere.
- Isochronous transfers: OUT only (issue #610), on the xHCI, EHCI, UHCI
  and OHCI, not the DWC2 (issue #608). Two endpoints per device on the
  xHCI; eight endpoints in all on an EHCI, UHCI or OHCI. An EHCI's service
  intervals are 4 frames apart at most. 4 USB speakers.
- No streams. No MSI for the devices: on aarch64 the controller's own
  interrupt is INTx (`docs/interrupts.md`).
- Bulk transfers go through a 64 KiB bounce buffer per controller, one at
  a time. A bulk IN endpoint whose transfers are queued (`transfer_start`)
  has a page of its own instead, so a transfer waiting for data holds
  nothing else. A control transfer's data stage is 1 KiB at most.
- `/dev/usb`: 8 devices, 4 interfaces and 16 endpoints each. An IN
  endpoint's transfer is a page (bulk) or a packet of up to 64 bytes
  (interrupt), and four completed ones wait to be read. No isochronous
  endpoints. One program per device.
- Real hardware: the BIOS handoff (`USBLEGSUP`) is done; the EHCI-to-xHCI
  port routing of older Intel chipsets is not. The Raspberry Pi 4's VL805
  is taken with the firmware its UEFI firmware loaded
  (`docs/raspberry-pi.md`). A controller whose DMA is not coherent gets
  the cache maintenance of its handle (`docs/pci.md` "DMA").
