# Power-off, reboot and halt

`poweroff`, `reboot` and `halt` (`/bin/custom`, one program, `user/power`)
take the system down. They are the native `power(action)` call, which needs
`write` on the kernel object `kernel.power`: in the default policy only the
`admin` domain (root) has it (`docs/security.md`). libc's `reboot(howto)`
(`<sys/reboot.h>`: `RB_AUTOBOOT`, `RB_HALT`, `RB_POWEROFF`) and, in the
Linux layer, Linux's `reboot(2)` are the same call.

## The sequence

The kernel does the whole of it, whatever init is (`kernel/src/power.rs`):

1. **The processes stop.** Every process but the caller gets `SIGTERM`; the
   kernel waits up to 2 seconds for them to exit, then sends `SIGKILL` to
   what is left and waits up to 1 second more. Kernel threads (a driver's
   own, like the xHCI `usb` thread) keep running.
2. **The disks are unmounted**, the deepest mount first, so each filesystem
   writes back what it caches (ext2). The binds go first: no process is
   left to use them. A mount still busy (an fd the caller holds there) is
   named on the console and left mounted. The block cache writes through
   and needs nothing.
3. **The methods are tried in order**, each announced on the console
   (`[ INFO ] power off via acpi s5`). A method that returns has failed;
   the next one is tried after a moment (a reset line takes a little while
   to act).
4. When none worked, or for `halt`, the console says `system halted` and
   the CPU stops; the machine stays on.

A second `power` call while the system is going down returns at once.

## The methods

The ones modules register (`KernelApi::power_register`, ABI 31) come first,
in the order they registered; then the arch's own
(`kernel/src/arch/*/power.rs`).

| Arch | Power off | Reboot |
|------|-----------|--------|
| x86_64 | ACPI S5 (`modules/acpi`) | the FADT's reset register (`modules/acpi`), the reset control register (port 0xCF9), the keyboard controller (0xFE to port 0x64), a triple fault |
| aarch64 | PSCI `SYSTEM_OFF` | PSCI `SYSTEM_RESET` |
| riscv64 | SBI `SRST` shutdown, SBI 0.1 shutdown | SBI `SRST` cold reboot |

- **ACPI** (`modules/acpi/src/power.rs`): S5 is entered by writing `_S5`'s
  sleep types with `SLP_EN` to the FADT's PM1a (and PM1b) control
  registers, after handing ACPI to the OS through the SMI command port
  when the firmware has not (`SCI_EN` clear). The `_S5` values come from
  the DSDT (the module's AML scan, `/proc/acpi/s5`). Only registers in
  system I/O space are used, which is what a PC has; a hardware-reduced
  platform (aarch64 under EDK2) has none and uses PSCI.
- **PSCI** is called through the conduit the platform description names:
  the device tree's `/psci` `method`, or the FADT's arm boot flags
  (`psci hvc (dt)` in `/proc/platform`); without one, HVC at EL1 and SMC
  at EL2.

`exit_qemu`, which the kernel calls on a panic and when the last process
exits, is not this sequence: it powers off at once (PSCI, SBI) or, on
x86_64, writes the test runs' `isa-debug-exit` port.

## Tests

- After the boot tests, the host types `poweroff` and expects QEMU to exit
  by itself, a `power off via` line on the console (`src/boot_test.rs`);
  the scratch disk's `e2fsck` that follows checks the unmount wrote ext2
  back.
- `sec_power` (`user/tests/kernel.sh`): a user without `kernel.power` is
  refused under every name.
- `platform`: an arm board names its PSCI conduit.
- The Linux smoke: `reboot(RB_DISABLE_CAD)` succeeds (Ctrl-Alt-Del has no
  meaning here; an init turns it off first), an unknown command is
  `EINVAL`.
