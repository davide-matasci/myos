# Power-off, reboot and halt

`poweroff`, `reboot` and `halt` take the system down. The kernel does the
work: it stops the processes, unmounts the disks and then tries each way it
knows to power off or reset the machine.

## Commands and calls

- `poweroff`, `reboot` and `halt` are one program (`user/power`), installed
  in `/bin/custom`.
- They make the native `power(action)` call. It needs `write` on the kernel
  object `kernel.power`. In the default policy only the `admin` domain
  (root) has it (`docs/security.md`).
- libc's `reboot(howto)` (`<sys/reboot.h>`: `RB_AUTOBOOT`, `RB_HALT`,
  `RB_POWEROFF`) is the same call.
- In the Linux layer, Linux's `reboot(2)` is the same call too.

## The sequence

The kernel runs the whole sequence, whatever init is (`kernel/src/power.rs`):

1. **The processes stop.** Every process but the caller gets `SIGTERM`.
   The kernel waits up to 2 seconds for them to exit. Then it sends
   `SIGKILL` to what is left and waits up to 1 second more. Kernel threads
   (a driver's own, like the xHCI `usb` thread) keep running.
2. **The disks are unmounted**, the deepest mount first, so each
   filesystem writes back what it caches (ext2). The binds go first, since
   no process is left to use them. A mount that is still busy (an fd the
   caller holds there) is named on the console and left mounted. The block
   cache writes through and needs nothing.
3. **The methods are tried in order.** The console announces each one
   (`[ INFO ] power off via acpi s5`). A method that returns has failed.
   The next one is tried after a moment, because a reset line takes a
   little while to act.
4. **The CPU stops.** When no method worked, or for `halt`, the console
   says `system halted` and the CPU stops. The machine stays on.

A second `power` call while the system is going down returns at once.

## The methods

Methods that modules register (`KernelApi::power_register`, ABI 31) come
first, in the order they registered. The arch's own methods
(`kernel/src/arch/*/power.rs`) follow.

| Arch | Power off | Reboot |
|------|-----------|--------|
| x86_64 | ACPI S5 (`modules/acpi`) | the FADT's reset register (`modules/acpi`), the reset control register (port 0xCF9), the keyboard controller (0xFE to port 0x64), a triple fault |
| aarch64 | PSCI `SYSTEM_OFF` | PSCI `SYSTEM_RESET` |
| riscv64 | SBI `SRST` shutdown, SBI 0.1 shutdown | SBI `SRST` cold reboot |

### ACPI

The ACPI module (`modules/acpi/src/power.rs`) enters S5 by writing `_S5`'s
sleep types with `SLP_EN` to the FADT's PM1a (and PM1b) control registers.

- If the firmware has not handed ACPI to the OS (`SCI_EN` clear), the
  module does so first, through the SMI command port.
- The `_S5` values come from the DSDT. The module's AML interpreter reads
  them, or its scan when the interpreter failed (`/proc/acpi/s5`,
  `docs/platform.md` "AML").
- Only registers in system I/O space are used, which is what a PC has. A
  hardware-reduced platform (aarch64 under EDK2) has none and uses PSCI.

### PSCI

PSCI is called through the conduit the platform description names: the
device tree's `/psci` `method`, or the FADT's arm boot flags.
`/proc/platform` shows it (`psci hvc (dt)`). Without one, the kernel uses
HVC at EL1 and SMC at EL2. On a Raspberry Pi 3 that HVC goes to myos's own
boot stub, a minimal PSCI (`docs/raspberry-pi.md`).

### `exit_qemu`

The kernel calls `exit_qemu` on a panic and when the last process exits.
It does not run this sequence. It powers off at once (PSCI, SBI) or, on
x86_64, writes the test runs' `isa-debug-exit` port.

## Tests

- After the boot tests, the host types `poweroff`. It expects QEMU to exit
  by itself and a `power off via` line on the console (`src/boot_test.rs`).
  The scratch disk's `e2fsck` that follows checks that the unmount wrote
  ext2 back.
- `sec_power` (`user/tests/kernel.sh`): a user without `kernel.power` is
  refused under every name.
- `platform`: an arm board names its PSCI conduit.
- The Linux smoke: `reboot(RB_DISABLE_CAD)` succeeds, and an unknown
  command is `EINVAL`. Ctrl-Alt-Del has no meaning here, but an init turns
  it off first, so the call must not fail.
