# The myos sysdeps of mlibc

`sysdeps/myos` is installed into the fetched mlibc tree by `patch.sh`
(`docs/mlibc.md` says how the whole fits together). Its ABI headers
(`include/abi-bits`) are checked in, where the Linux ones mlibc carries
(`abis/linux` in its tree, which `patch.sh` links for the rest) do not fit
the kernel. On a bump of `MLIBC_TAG`, diff them against the new upstream
files and carry the upstream changes over:

- `signal.h`: upstream's `abis/linux/signal.h` with the kernel's numbers
  (newlib's BSD layout, `kernel/src/signal.rs`: `SIGURG 16`, `SIGSTOP 17`,
  `SIGTSTP 18`, `SIGCONT 19`, `SIGCHLD 20`, `SIGIO 23`, `SIGLOST 29`,
  `SIGUSR1 30`, `SIGUSR2 31`, `SIGEMT 7`, `SIGBUS 10`, `SIGSYS 12`), the
  kernel's `sa_flags` (`SA_SIGINFO 2`, `SA_ONSTACK 4`, `SA_NOCLDWAIT 0x20`;
  `SA_RESTORER` is not the kernel's: the trampoline is `libmyos`'s),
  `_NSIG 32` (32-bit masks), `SI_USER 1` (what the kernel writes for
  `kill`), `SIGCANCEL` as `SIGLOST` (what mlibc's `pthread_cancel` sends,
  as libgloss's does), and `SIGPWR`, `SIGRTMIN`, `SIGRTMAX` past `NSIG`
  (the kernel has neither; the code that names them compiles).
- `termios.h`: the flag bits and control character indexes the kernel's
  line discipline reads (`docs/tty.md`), which are newlib's
  (`toolchain/newlib/libgloss/myos/termios.h`), the speeds as indexes.
- `ioctls.h`: the requests with Linux's numbers; none reaches the kernel
  (`libmyos/ioctl.c` serves the terminal ones from the terminal's files).
- `vm-flags.h`: upstream's `abis/linux/vm-flags.h` with `memfd_create`'s
  flags outside the Linux option, which myos does not build.

`include/mlibc/sysdeps.hpp` names the sysdeps the `generic/*.cpp` files
implement; a new one is added to both.
