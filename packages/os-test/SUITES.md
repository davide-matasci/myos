# os-test suite inventory (pinned rev in `versions.env`)

Upstream [sortix/os-test](https://gitlab.com/sortix/os-test) suites present in
`target/os-test-embed/` after `fetch.sh`:

| Suite | `*.c` count | In default `SUITES`? | Notes |
|-------|------------:|:--------------------:|-------|
| **basic** | 1187 | yes | libc/syscall smoke; CI thin list in `misc/ci-basic-smoke.tests` |
| **limits** | 126 | yes | `limits.h` constant values |
| **io** | 55 | yes | open/fcntl OFD locks, tmpdir opens, CLOFORK |
| **malloc** | 3 | yes | `malloc(0)` / `realloc` edge cases |
| **paths** | 48 | yes | filesystem path / device presence |
| **process** | 24 | yes | fork/setpgid/setsid/waitpid process-group |
| **signal** | 32 | yes | raise/block/ignore/sigaltstack/ppoll/exec; handlers are kernel-delivered (`docs/signals.md`) |
| **stdio** | 22 | yes | printf formatting |
| **namespace** | 159 | no | header pollution vs include-suite APIs (heavy tooling) |
| **pty** | 29 | no | pseudoterminals / controlling tty |
| **udp** | 207 | yes | UDP socket semantics |
| **myos** | 12 | yes | myos overlay (`overlay/myos/`): `chroot(2)` + named FIFOs |
| **posix-parse** | 1 | no | parser helper |
| **include** | 3758 | no | API declaration corpus (not runtime tests) |

**Non-basic** = everything except `basic` (and the non-runtime `include` /
`posix-parse` helpers). Curated boot CI set: `misc/ci-nonbasic-100.tests`
(~100 paths, suite-prefixed) plus `misc/ci-expansion.tests` (155 paths:
POSIX core + non-basic + the myos suite) and `misc/ci-expansion-2.tests`
(503 paths: the rest of the suite that passes) and `misc/ci-udp.tests` (the
udp suite's 207). Wired the same way as basic: thin
`ci-smoke-copy.sh` staging + host prebuild + `make … TESTLIST=… report`.

## `ci-expansion.tests` (POSIX core + non-basic + myos)

Picked from a guest triage of every buildable candidate in `basic/`
(dirent, fcntl, sys_stat, unistd, select/poll/wait, signal, stdio, stdlib,
time) and the `process`, `signal`, `io`, `paths`, `limits` suites; only tests
that pass on the guest are listed. Support added for them: `chroot(2)` and
named FIFOs in the kernel; `mkfifo`/`mkfifoat`/`mknod(S_IFIFO)`/`mknodat`,
`chroot`, `dup3`, `pipe2`, `pread`/`pwrite`, `fsync`/`fdatasync`,
`truncate`, `fchdir`, `mkdirat`/`renameat`/`readlinkat`, `execv`/`execl`/
`execle`, `killpg`, `posix_memalign`, `flockfile` & co, `telldir`/`seekdir`/
`dirfd`/`rewinddir`, `scandir`/`alphasort` and the `<limits.h>` POSIX/XSI
minima in libgloss.

The **myos** suite (`overlay/myos/`) covers what upstream does not:
`chroot` (basic confinement, `..` at the new root, cwd handling, inheritance
across fork, nested chroot, error cases) and named FIFOs (`stat` type,
`EEXIST`/`ENOENT`, blocking rendezvous + EOF, `O_NONBLOCK` open semantics,
re-use across sessions, `mknod(S_IFIFO)` / `mkfifoat`).

## `ci-expansion-2.tests` (the rest that passes)

Every runtime test outside the other lists was host-built for all three
arches and booted in batches; the list keeps the 503 that build and pass on
bios, aarch64 and riscv64: most of `basic/` (wchar, wctype, pthread, stdlib,
stdio, unistd, time, fenv, complex, ndbm, pwd/grp, locale, sys_mman,
sys_shm, termios, syslog, ...), the `paths` FHS checks (a missing directory
is one of their expected outcomes), the `process/fork-setpgid-*undo*` tests
and a few `stdio` printf cases. Of the 1352 candidates, 632 build against
newlib + libgloss on all three arches today; what fails or does not build is
below.

## `ci-udp.tests` (the udp suite)

All 207 tests of `udp/`: binding (port 0, conflicts and `SO_REUSEADDR`,
loopback, LAN and broadcast addresses), connect and unconnect, reconnects,
`getsockname`/`getpeername`, `sendto`/`recvfrom` between two or three
sockets, `shutdown`, `poll`, and the ECONNREFUSED a datagram to a closed
port gives a connected socket (the ICMP "port unreachable" netd's loopback
interface sees). The LAN tests find the interface with `getifaddrs`. Many
tests wait 50 ms for that ICMP error: the boot tests run them under TCG
all the same.

## Deferred from curated set (implemented but not yet green)

These have real support started in-tree but are **not** in `ci-nonbasic-100.tests`
until they pass honestly (no xfails):

- `io/open-clofork-fork`, `io/dup3-clofork-fork` — kernel `fd_clofork_mask` +
  libgloss O_CLOFORK; still failing child fstat after fork on CI
- `process/fork-setpgid-on-parent`, `-on-parent-move`, `-invalid`,
  `limbo-setpgid` — pgid edge cases still red; `limbo-getpgid` passes on
  x86_64 and aarch64 but not riscv64 (issue #375)
- `process/waitpid-pgid`, `waitpid-pgid-empty-on-setsid` — need
  waitpid(pgid) filtering (waitpid currently ignores pid)
- `basic/signal/sigismember`, `sigaddset`, `sigdelset` — newlib's macros
  shift by the (negative / huge) signal number unchecked; clang turns that UB
  into a trap. newlib's checked versions (`libc/unix/sigset.c`) use bit
  `signo - 1`, unlike the macros and the kernel (bit `signo`), so they cannot
  simply be swapped in.
- `io/ofd-*` — open-file-description locks (`F_OFD_SETLK`/`F_OFD_GETLK`) are
  not implemented; `io/ofd-setlk-wr-dup-rd` hangs
- `stdio/printf-c-pos-args` — POSIX's numbered arguments (`%3$c`) need
  newlib's `--enable-newlib-io-pos-args`, whose `get_arg` takes `&ap` of a
  `va_list` parameter: on x86_64 that is an array type, decayed to a
  pointer, and the program faults (CI run on PR #259). Without the option
  the conversions print literally
- `process/zombie-setpgid-move` (hang)
- `basic/complex/*` (25) — results differ from the expected ones in the
  last bits; `basic/unistd` (16), `basic/sys_socket` (13), `basic/stdlib`
  (5) and a few more in `stdio`, `sys_stat`, `wchar`, `netdb`, `time`,
  `sys_wait`, `sys_times`, `sys_select`, `sys_shm`, `pthread`, `locale` —
  missing or partial libc/kernel support
- `limits/*` (35) — the POSIX/XSI limit macros newlib does not define
  (`PAGESIZE`, `HOST_NAME_MAX`, `PTHREAD_*`, `SEM_*`, `AIO_*`, `NL_*`, ...)
- `process/waitpid-pgid-empty-on-setpgid*`, `stdio/printf-Lf-width-precision-pos-args`
- Not buildable against newlib/libgloss on every arch yet (not in any
  list; 720 of the 1352 candidates; 825 build for x86_64, 750 for aarch64,
  632 for riscv64): most of `basic/math` (174: riscv64imac is soft-float and
  its `<fenv.h>` has no `FE_*` exception flags), `basic/threads`, `spawn`,
  `semaphore`, `mqueue`, `aio`, `sched`, `libintl`, `utmpx`, `dlfcn`,
  `iconv`, `wordexp`, the pty API
  (`posix_openpt`/`grantpt`/`unlockpt`), `ppoll`, `timer_*`,
  `getppid`, `SA_ONSTACK`/`sigaltstack`, `sigqueue`, `sigtimedwait` /
  `sigwaitinfo`, `siginfo_t.si_pid`, `struct rlimit`

- **udp/** entries were removed from `ci-nonbasic-100.tests` on 2026-09-21
  (below); the whole suite is curated in `ci-udp.tests` since netd has a
  loopback interface and libgloss full UDP sockets (`docs/sockets-curl.md`).

## 2026-09-21 networking regression rollback

Libgloss/kernel additions for curated non-basic (CLOFORK, getifaddrs, limits
macros, userspace raise/sigaltstack, ppoll, getppid, UDP bind/getsockname,
sleep elapsed-time) regressed aarch64 `curl` TLS and riscv64 dropbear SSH
even after restoring master `socket.c`. Those libc/kernel files are restored
to master; curated `ci-nonbasic-100.tests` keeps suites that do not need them
(malloc/paths/stdio + remaining honest peers). Re-land support suite-by-suite
with curl+SSH held green on aarch64/riscv64 full boot.

