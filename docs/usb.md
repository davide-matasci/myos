# USB

Five modules give myos a USB bus: `usb`, the core, owns the devices and
publishes the bus; `xhci` drives the host controllers and registers them
with the core; `usb_hub` makes the devices behind hubs reachable,
`usb_hid` feeds mice and tablets to the console's pointer
(`/dev/console/mouse`, `docs/tty.md`) and keyboards to its keyboard
input, and
`usb_storage` turns memory sticks and disk enclosures into `/dev/sdX/`
(`data`, the whole disk, `p<N>` per GPT partition, and `info`: the
disk's size, its `/proc/usb` device and vendor:product, and the vendor
and model its INQUIRY answer gives). A device plugged
in after boot is enumerated, a device pulled out is gone
from `/dev`; `/proc/usb` lists what is there. Every CI boot carries a
controller, a hub, a mouse and two sticks (one plugged in by a test),
and a test plugs in a keyboard.

| Module | What |
|--------|------|
| `modules/usb` | The core: the device table, enumeration (descriptors, configuration), the class drivers and the interfaces offered to them, hubs' children and their route strings, root-port hot-plug on its own kernel thread, the `UsbHostOps` service, `/proc/usb`; the host controllers' `UsbCoreOps` service |
| `modules/xhci` | The xHCI controller driver (PCI class `0C/03`, interface `30`): rings, interrupter, device slots and contexts, the transfers; registered with the core as a `UsbHcdOps` |
| `modules/usb_hub` | Hub class (9): hub descriptor, port power and reset, the status-change interrupt endpoint; children through `hub_attach` / `hub_detach` |
| `modules/usb_storage` | Mass storage class (8/6/0x50, bulk-only SCSI): `blk_register` as `sd?` (the kernel hands out the letter, shared with the virtio-scsi disks); `blk_unregister` on unplug |
| `modules/usb_hid` | HID class (3) pointers: the report descriptor's X and Y axes, wheel and buttons (or the boot mouse report), its interrupt endpoint's reports into the console's `pointer` service; boot keyboards (3/1/1): the boot report's keys into the console's `keyboard` service, its Caps Lock LED from the console |

## How the modules reach each other

Modules call only the `KernelApi`, never each other, so ABI 21 added a
**service registry**: `service_register(name, table)` publishes a
`#[repr(C)]` function table, `service_lookup(name)` finds it. The core
publishes [`UsbHostOps`](../modules/abi/src/lib.rs) as `usb`; a class
driver looks it up in its `module_init` and registers a `UsbDriverOps`
(`probe`, `disconnect`) with `driver_register`. A lookup counts as a
registration, so a class driver stays loaded while the core may call it;
the core stays loaded because of its services and its thread (`rmmod`
refuses both).

The core also publishes `UsbCoreOps` as `usbcore`, for the **host
controller drivers**: a controller module (`xhci`; `dwc2` for the
Raspberry Pi 3's) brings its controllers up and registers each with
`controller_register`, handing the core a `UsbHcdOps` table, the
controller's part of the bus: its root ports (`ports`, `port_status`,
`port_reset`), the per-device state the controller keeps (`device_new`
for a slot or an address record, `device_address`, `device_ep0`,
`device_configure`, `device_hub`, `device_abort`, `device_free`) and the
transfers (`control`, `bulk`, `interrupt_start` and `interrupt_take`,
`clear_halt`). The core calls a controller's table one call per device at
a time, holding the device's lock across it; the controller's interrupt
handler calls the core's `port_change` and `wake`, which only flag and
wake the thread. A controller without an interrupt says so from `poll`
(every pass of the thread), which then runs every 20 ms. Adding a
controller type is a module that implements `UsbHcdOps`; the core, the
class drivers and `/proc/usb` are the same above every one, and
controllers of several types run side by side.

The core enumerates on **its own kernel thread** (`thread_spawn`, another
ABI 21 entry): resets, descriptor reads and `SET_CONFIGURATION` take
milliseconds and wait on completions, which an interrupt handler cannot
do. Every driver hook (`probe`, `disconnect`, the completion callback of
`interrupt_start`) runs on that thread too, so a hub's `probe` may reset
its ports and enumerate its children synchronously through `hub_attach`.
Transfers started by a class driver from another context (a `blk_read`
from a user task) run there: a device's table entry is behind a
`myos_abi::SleepLock` the transfer holds until the controller driver
returns (the thread holds it while it enumerates the device). The core
holds no device lock across a driver hook: `probe`, `disconnect` and a
completion callback call back in. A device pulled out has its transfers
in flight failed by the controller (`device_abort`, `USB_EGONE`) before
`detach` waits for its lock, so the lock comes free at once; a queued
transfer gets the error in its callback (`interrupt_take` hands it over),
and the controller's records are reset with the device (`device_free`),
so the next device in the entry starts clean.

In `xhci`, the command ring and the bulk bounce buffer are locked, and the
completions are atomics outside the device's lock, which the interrupt
handler fills and `wake`s (the last ABI 21 entry) the waiter with.
Without an interrupt (`pci_irq_enable` failed) the bus still works: a
waiter polls the event ring every 2 ms, the core's thread every 20 ms.
The controller itself is reached as `&Controller` from every context:
its command ring and bulk bounce buffer are each behind a `SleepLock`,
the event ring belongs to whoever holds its busy flag (the handler, or a
waiter polling it), and the rest is atomics or written by one context
alone. The one that finds the ring busy still acknowledges the
interrupter (`IMAN.IP`) and asks the holder to look again once it lets
go: on aarch64 and riscv64 the interrupt is a level INTx line, which
would fire again at once and, on the CPU whose waiter it interrupted,
never let that waiter finish (`hc::process_events`).
The locks nest in one order: a class driver's own slot lock (a disk's),
then the core's device entry, then the controller's (xhci's device entry,
then its command ring or bounce buffer). The core never holds a device
lock across a driver hook, so the reverse never happens, and the
interrupt handler holds none.

## Enumeration

A root port's connect event (or the initial scan) wakes the thread: it
debounces 100 ms, resets a USB 2 port (a USB 3 link trains itself), reads
the port speed, and runs the enumeration every new device goes through:

1. `device_new` with the device's path: the route string (the port at
   every hub tier, 4 bits each), the speed, the root port, the
   transaction translator (a low/full-speed device behind a high-speed
   hub: that hub and port) and the address the core gives it (xHCI:
   `Enable Slot`, the slot's output device context into the DCBAA,
   endpoint 0's ring at the speed's default max packet size).
2. `device_address` (xHCI: `Address Device` with the input context; a
   controller without that command sends `SET_ADDRESS`).
3. The device descriptor, 8 bytes first: a full-speed device may use
   another max packet size for endpoint 0 (`device_ep0`; xHCI:
   `Evaluate Context`).
4. The first configuration descriptor: its interfaces (alternate setting
   0) and their endpoints; isochronous endpoints are skipped.
5. `device_configure` with every endpoint (xHCI: `Configure Endpoint`,
   one transfer ring each), `SET_CONFIGURATION`.
6. Every interface is offered to the class drivers in registration order
   until one takes it; a driver registered later is offered the unclaimed
   interfaces of every device.

A hub's `probe` reads the hub descriptor, tells the core the port count
(`hub_configure`; xHCI: the slot context's hub fields), powers the ports, and for
every connected port resets it and calls `hub_attach(hub, port, speed)`,
which runs the same six steps as the hub's child (depth + 1 in the route
string). Its status-change endpoint then reports connects and disconnects;
`hub_detach(hub, port)` tears a child down: its own children first (a
nested hub), its waiters failed with `USB_EGONE` (a callback transfer's
callback runs with it), its drivers' `disconnect`, `device_free` (xHCI:
`Disable Slot`, its memory back to the module's pool).

`usb_hid` claims an interface of class 3 whose HID report descriptor
(`GET_DESCRIPTOR` of the interface, type `0x22`) has an X and a Y axis of
the generic desktop page: a mouse's are relative, a tablet's absolute
(QEMU's `usb-tablet`, scaled to the pointer's 0–32767). It reads the
report with those axes (by its report id, when the device numbers its
reports), the vertical wheel and buttons 1 to 5 (left, right, middle,
side, extra); a boot mouse whose descriptor does not parse is switched to
the boot protocol (`SET_PROTOCOL`) and its fixed three-byte report. It
asks for reports only on change (`SET_IDLE` 0), keeps one transfer queued
on the interrupt IN endpoint (`interrupt_start`) and queues the next from
the completion, on the host's thread; motion goes to the console as it
comes (a tablet's position when it moved), buttons as presses and
releases of what changed.

A boot keyboard's interface (class 3, subclass 1, protocol 1) is switched
to the boot protocol, whatever its descriptor says: every keyboard has
it, and its eight-byte report is fixed (the modifier bits, then up to six
keys held, as usages of the Keyboard/Keypad page). What changed from the
last report goes to the console's `keyboard` service as presses and
releases in Linux `KEY_*` codes (`modules/usb_hid/src/keyboard.rs` maps
the usages; a report of ErrorRollOver, more keys held than it can name,
changes nothing), where the console's own keyboard's go (`docs/tty.md`).
A USB keyboard does not repeat a held key; the console does. After each
report, and when the keyboard is plugged in, `usb_hid` asks the console
which lock LEDs to light (Caps Lock) and sends the keyboard an output
report (`SET_REPORT`) when that changed. A keyboard pulled out releases
the keys it held.

`usb_storage` claims an interface of class 8, subclass 6, protocol `0x50`
with a bulk IN and a bulk OUT endpoint, runs `TEST UNIT READY` until the
unit is (sticks need a moment) and `READ CAPACITY(10)`, refuses a sector
size other than 512 bytes, and registers the disk. `READ(10)` /
`WRITE(10)` move each request as one command: CBW out, data, CSW in; a
stalled endpoint is cleared (`clear_halt`), a failed command gets a
`REQUEST SENSE` and one retry, a phase error a bulk-only reset.

## Hot-plug and unplug

Plug: a root port's change event or a hub's status report, then the
enumeration above. Unplug: the block device is unregistered
(`blk_unregister`) when nothing holds it; while a filesystem is mounted
from it or an fd is open on it the kernel refuses (`MYOS_EBUSY`) and the
entry stays in `/dev`, failing its I/O, until the disk is unmounted
(`umount`) and the fds are closed. `/proc/usb` marks such a device
`gone`.

## `/proc/usb`

One line per device:

```
controller 0 ports 8 irq on events 86 irqs 83
0x001 parent 0x000 port 5 full 0409:55aa class 09 if0=09/00/00:usb_hub hub 8
0x002 parent 0x001 port 1 full 46f4:0001 class 00 if0=08/06/50:usb_storage sdb
0x003 parent 0x000 port 7 high 0627:0001 class 00 if0=03/01/02:usb_hid mouse
0x004 parent 0x000 port 2 super 46f4:0001 class 00 if0=08/06/50:usb_storage sdc
```

a line per controller (its root ports, then what its driver says: whether
its interrupt is on, the events and interrupts seen), then one per device: the id (controller in
the high byte), the parent hub (0: a root port) and port, the speed,
vendor:product, the device class and each interface's class/subclass/
protocol with the driver that took it and what the driver made of it (a
disk: its name in `/dev`, the label `usb_storage` hands the core through
`interface_label` once the disk is registered; a pointer `mouse` or
`tablet`), a hub its port count. The
port path (parent and port, up to the root) says where a disk is plugged
in.

## Testing

The launcher gives every boot `qemu-xhci`, a `usb-hub` on its `port=1`
(a USB 2 port, number 5 of the controller's eight: QEMU's hub is
full-speed, and so is the stick behind it) and a `usb-storage` behind the
hub (`/dev/sdb/data`, the same FAT volume as `/dev/vda/data` in its own image file),
plus a second stick's drive that `user/tests/host.sh usb-plug` attaches to
`port=2` (a USB 3 port: a SuperSpeed device) through the QEMU monitor and
`usb-unplug` removes. `kernel.sh` waits for `/dev/sdb/data`,
checks `/proc/usb`, mounts and reads the volume (`usb_disk`), then plugs,
reads and unplugs the second stick (`usb_hotplug`). A test boot also has
a `usb-mouse` on `port=3` (number 7 of the controller's, a high-speed
device), which `user/c/test.sh`'s `mouse` test moves and
clicks through the monitor (`user/c/host.sh mouse`: `mouse_move`,
`mouse_button`) while `mouse_smoke` reads `/dev/console/mouse`;
`packages/x11-apps` checks the same through Xfbdev and xev. The
`usb_keyboard` test of `user/c/test.sh` has the host plug a `usb-kbd` into
`port=4` (`user/c/host.sh usb-kbd plug`), which then takes the monitor's
`sendkey` (QEMU types on the keyboard added last), checks
`/dev/console/kbd`'s events, a line typed on tty9 with Caps Lock and a
held key's repeats, that the console's own keyboard's interrupt count did
not move, and the unplug. A boot in a
window (`cargo run` without a test) has a `usb-tablet` there instead,
which follows the host's pointer without grabbing it (the monitor's
`mouse_move` is relative, so the tests use the mouse). The boot markers
`[ OK ] usb`, `xhci`, `usb_hub`, `usb_storage`, `usb_hid` cover the
modules' init.

## Limits

- Four controllers (two xHCI ones), 32 devices each, hubs five deep, 8
  hubs, 8 disks and 8 HID devices, pointers and keyboards together (the
  class modules' tables); four interrupt transfers queued per device.
- A pointer's report descriptor is read up to 512 bytes and its reports
  up to 64; no horizontal wheel, no buttons past the fifth. One pointer
  per interface: the reports of a composite device's other collections (a
  keyboard's) are skipped.
- xHCI controllers on PCI only (the Raspberry Pi 3's DWC2 is under way);
  a SoC's `snps,dwc3` from the device tree would be another controller
  module.
- No isochronous transfers, no streams, no MSI for the devices: on
  aarch64 the controller's own interrupt is INTx (`docs/pci-acpi-smp.md`).
- Bulk transfers go through a 64 KiB bounce buffer per controller, one at
  a time; a control transfer's data stage is 1 KiB at most.
- Keyboards through the boot protocol only: six keys at once (more is
  ErrorRollOver), no media or system keys (they are another interface,
  in the report protocol, which `usb_hid` does not take), and only the
  Caps Lock LED (the console has no Num or Scroll Lock state). The LED
  report is not checked by a test: QEMU shows it nowhere.
- Real-hardware notes: the BIOS handoff (`USBLEGSUP`) is done; the
  EHCI-to-xHCI port routing of older Intel chipsets is not. The
  Raspberry Pi 4's VL805 is taken with the firmware its UEFI firmware
  loaded (`docs/raspberry-pi.md`); a controller whose DMA is not coherent
  gets the cache maintenance of its handle (`docs/pci-acpi-smp.md` "DMA").
