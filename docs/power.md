# Power-off, reboot and halt

`poweroff`, `reboot` and `halt` take the system down. The kernel does the
work (`kernel/src/power.rs`): it stops the processes, unmounts the disks
and then tries each way it knows to power off or reset the machine.

## Commands and calls

- `poweroff`, `reboot` and `halt` are one program (`user/power`), installed
  in `/bin/custom`.
- They make the native `power(action)` call. It needs `write` on the kernel
  object `kernel.power`, which in the default policy only the `admin`
  domain (root) has (`docs/security.md`).
- libc's `reboot(howto)` (`<sys/reboot.h>`) and, in the Linux layer,
  Linux's `reboot(2)` are the same call.

## The sequence

The kernel runs it whatever init is:

1. Every process but the caller gets `SIGTERM`, then `SIGKILL` after 2
   seconds. Kernel threads keep running.
2. The disks are unmounted, the deepest mount first, so each filesystem
   writes back what it caches (ext2). A mount that is still busy is named
   on the console and left mounted.
3. The methods are tried in order. The console announces each one
   (`[ INFO ] power off via acpi s5`). A method that returns has failed.
4. When no method worked, or for `halt`, the console says `system halted`
   and the CPU stops. The machine stays on.

A second `power` call while the system is going down returns at once.

## The methods

Methods that modules register (`KernelApi::power_register`) come first,
then the arch's own (`kernel/src/arch/*/power.rs`).

| Arch | Power off | Reboot |
|------|-----------|--------|
| x86_64 | ACPI S5 (`modules/acpi`) | the FADT's reset register (`modules/acpi`), port 0xCF9, the keyboard controller, a triple fault |
| aarch64 | PSCI `SYSTEM_OFF` | PSCI `SYSTEM_RESET` |
| riscv64 | SBI `SRST` shutdown, SBI 0.1 shutdown | SBI `SRST` cold reboot |

- **ACPI** (`modules/acpi/src/power.rs`) takes the `_S5` sleep types from
  the DSDT through the AML interpreter (`/proc/acpi/s5`,
  `docs/platform.md` "AML"). It uses only registers in system I/O space,
  which is what a PC has. A hardware-reduced platform (aarch64 under EDK2)
  uses PSCI.
- **PSCI** is called through the conduit the device tree or the FADT
  names; `/proc/platform` shows it (`psci hvc (dt)`). On a Raspberry Pi 3
  the HVC goes to myos's own boot stub (`docs/raspberry-pi.md`).
- `exit_qemu`, called on a panic and when the last process exits, skips
  this sequence and powers off at once.

## Tests

After the boot tests the host types `poweroff` and expects QEMU to exit
with a `power off via` line on the console (`src/boot_test.rs`); the
scratch disk's `e2fsck` then checks the unmount. `sec_power`
(`user/tests/kernel.sh`) checks that a user without `kernel.power` is
refused.
