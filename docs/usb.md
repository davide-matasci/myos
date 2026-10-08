# USB

Three modules give myos a USB bus: `xhci` drives the host controller and
publishes the bus, `usb_hub` makes the devices behind hubs reachable,
`usb_storage` turns memory sticks and disk enclosures into `/dev/sdX/`
(`data`, the whole disk, and `p<N>` per GPT partition). A device plugged
in after boot is enumerated, a device pulled out is gone
from `/dev`; `/proc/usb` lists what is there. Every CI boot carries a
controller, a hub and two sticks (one plugged in by a test).

| Module | What |
|--------|------|
| `modules/xhci` | The xHCI controller (PCI class `0C/03`, interface `30`): rings, interrupter, device slots, enumeration, root-port hot-plug, the `UsbHostOps` service, `/proc/usb` |
| `modules/usb_hub` | Hub class (9): hub descriptor, port power and reset, the status-change interrupt endpoint; children through `hub_attach` / `hub_detach` |
| `modules/usb_storage` | Mass storage class (8/6/0x50, bulk-only SCSI): `blk_register` as `sda`, `sdb`, …; `blk_unregister` on unplug |

## How the modules reach each other

Modules call only the `KernelApi`, never each other, so ABI 21 added a
**service registry**: `service_register(name, table)` publishes a
`#[repr(C)]` function table, `service_lookup(name)` finds it. The host
publishes [`UsbHostOps`](../modules/abi/src/lib.rs) as `usb`; a class
driver looks it up in its `module_init` and registers a `UsbDriverOps`
(`probe`, `disconnect`) with `driver_register`. A lookup counts as a
registration, so a class driver stays loaded while the host may call it;
the host stays loaded because of its service, its thread and its interrupt
(`rmmod` refuses all three).

The host enumerates on **its own kernel thread** (`thread_spawn`, another
ABI 21 entry): resets, descriptor reads and `SET_CONFIGURATION` take
milliseconds and wait on completions, which the interrupt handler cannot
do. Every driver hook (`probe`, `disconnect`, the completion callback of
`interrupt_start`) runs on that thread too, so a hub's `probe` may reset
its ports and enumerate its children synchronously through `hub_attach`.
Transfers started by a class driver from another context (a `blk_read`
from a user task) run there: a device's table entry is behind a
`myos_abi::SleepLock` the transfer holds until its completion (the
thread holds it while it enumerates the device), the command ring and
the bulk bounce buffer are locked, and the completions are atomics
outside the entry's lock, which the interrupt handler fills and `wake`s
(the last ABI 21 entry) the waiter with. The host holds no device lock
across a driver hook: `probe`, `disconnect` and a completion callback
call back in. A device pulled out has its waiters failed (`USB_EGONE`)
before `detach` waits for its lock, so the lock comes free at once; a
transfer with a callback gets the error in its callback, and the entry's
completion records are reset with it, so the next device in the entry
starts clean.
Without an interrupt (`pci_irq_enable` failed) the bus still works: a
waiter polls the event ring every 2 ms, the thread every second.

The controller itself is reached as `&Controller` from every context:
its command ring and bulk bounce buffer are each behind a `SleepLock`,
the event ring belongs to whoever holds its busy flag (the handler, or a
polling waiter), and the rest is atomics or written by one context alone.
The locks nest in one order: a class driver's own slot lock (a disk's),
then the host's device entry, then the command ring or the bounce
buffer. The host never holds a device lock across a driver hook, so the
reverse never happens, and the interrupt handler holds none.

## Enumeration

A root port's connect event (or the initial scan) wakes the thread: it
debounces 100 ms, resets a USB 2 port (a USB 3 link trains itself), reads
the port speed, and runs the enumeration every new device goes through:

1. `Enable Slot`; the slot's output device context goes into the DCBAA.
2. `Address Device` with an input context naming the route string (the
   port at every hub tier, 4 bits each), the speed, the root port, the
   transaction translator (a low/full-speed device behind a high-speed
   hub: that hub's slot and port) and endpoint 0 with the speed's default
   max packet size.
3. The device descriptor, 8 bytes first: a full-speed device may use
   another max packet size for endpoint 0 (`Evaluate Context` then).
4. The first configuration descriptor: its interfaces (alternate setting
   0) and their endpoints; isochronous endpoints are skipped.
5. `Configure Endpoint` for every endpoint (one transfer ring each),
   `SET_CONFIGURATION`.
6. Every interface is offered to the class drivers in registration order
   until one takes it; a driver registered later is offered the unclaimed
   interfaces of every device.

A hub's `probe` reads the hub descriptor, tells the host the port count
(`hub_configure`: the slot context's hub fields), powers the ports, and for
every connected port resets it and calls `hub_attach(hub, port, speed)`,
which runs the same six steps as the hub's child (depth + 1 in the route
string). Its status-change endpoint then reports connects and disconnects;
`hub_detach(hub, port)` tears a child down: its own children first (a
nested hub), its waiters failed with `USB_EGONE` (a callback transfer's
callback runs with it), its drivers' `disconnect`, `Disable Slot`, its
memory back to the module's pool.

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
0x002 parent 0x001 port 1 full 46f4:0001 class 00 if0=08/06/50:usb_storage sda
0x003 parent 0x000 port 2 super 46f4:0001 class 00 if0=08/06/50:usb_storage sdb
```

a line per controller (its root ports, whether its interrupt is on, the
events and interrupts seen), then one per device: the id (controller in
the high byte), the parent hub (0: a root port) and port, the speed,
vendor:product, the device class and each interface's class/subclass/
protocol with the driver that took it and what the driver made of it (a
disk: its name in `/dev`, the label `usb_storage` hands the host through
`interface_label` once the disk is registered), a hub its port count. The
port path (parent and port, up to the root) says where a disk is plugged
in.

## Testing

The launcher gives every boot `qemu-xhci`, a `usb-hub` on its `port=1`
(a USB 2 port, number 5 of the controller's eight: QEMU's hub is
full-speed, and so is the stick behind it) and a `usb-storage` behind the
hub (`/dev/sda/data`, the same FAT volume as `/dev/vda/data` in its own image file),
plus a second stick's drive that `user/tests/host.sh usb-plug` attaches to
`port=2` (a USB 3 port: a SuperSpeed device) through the QEMU monitor and
`usb-unplug` removes. `kernel.sh` waits for `/dev/sda/data`,
checks `/proc/usb`, mounts and reads the volume (`usb_disk`), then plugs,
reads and unplugs the second stick (`usb_hotplug`). The boot markers
`[ OK ] xhci`, `usb_hub`, `usb_storage` cover the modules' init.

## Limits

- Two controllers, 32 devices each, hubs five deep, 8 hubs and 8 disks
  (the class modules' tables).
- PCI controllers only: a SoC's `snps,dwc3` from the device tree would need
  an `irq_register` for non-PCI interrupts (not in the ABI yet).
- No isochronous transfers, no streams, no MSI for the devices: on
  aarch64 the controller's own interrupt is INTx (`docs/pci-acpi-smp.md`).
- Bulk transfers go through a 64 KiB bounce buffer per controller, one at
  a time; a control transfer's data stage is 1 KiB at most.
- No USB keyboard yet: the console module's keyboards are PS/2 and
  virtio-input; a HID driver would be another class module.
- Real-hardware notes: the BIOS handoff (`USBLEGSUP`) is done; the
  EHCI-to-xHCI port routing of older Intel chipsets and the Raspberry
  Pi 4's VL805 firmware load are not.
