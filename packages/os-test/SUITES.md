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
(~100 paths, suite-prefixed) plus `misc/ci-expansion.tests` (150 paths:
POSIX core + non-basic + the myos suite). Wired the same way as basic: thin
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

## Deferred from curated set (implemented but not yet green)

These have real support started in-tree but are **not** in `ci-nonbasic-100.tests`
until they pass honestly (no xfails):

- `io/open-clofork-fork`, `io/dup3-clofork-fork` — kernel `fd_clofork_mask` +
  libgloss O_CLOFORK; still failing child fstat after fork on CI
- `process/fork-setpgid-*-undo*`, `fork-setpgid-on-parent`, `limbo-getpgid` —
  pgid edge cases still red
- `udp/connect-reconnect*`, `connect-unconnect-getpeername` — peer/unconnect edge cases
- `process/waitpid-pgid` — needs waitpid(pgid) filtering (waitpid currently
  ignores pid)
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
- `process/fork-setsid-setpgid`, `-in-parent`, `-move` — a session leader
  cannot change its process group (`EPERM`); libgloss's `setsid` only starts
  a process group (issue #273), so there is no session leader to refuse
- `process/fork-exec-setpgid-in-parent` — a parent cannot move a child that
  has exec'd (`EACCES`); the kernel does not remember the exec, and its one
  `setpgid` failure is `EPERM`
- `process/zombie-setpgid-move` (hang), `process/limbo-*`,
  `process/fork-setpgid-*undo*`/`-invalid` — pgid edge cases
- `paths/*` FHS directories (`/var`, `/run`, `/usr/share`, `/sbin`, …) and
  `/dev/{fd,stdin,stdout,stderr,full}` — not present in the image
- `basic/stdlib/strtod` — passes on x86_64, fails on riscv64 (cause not yet
  investigated)
- Not buildable against newlib/libgloss yet (not in any list): pty API
  (`posix_openpt`/`grantpt`/`unlockpt`), `ppoll`, `timer_*`,
  `getppid`, `SA_ONSTACK`/`sigaltstack`, `sigqueue`, `sigtimedwait` /
  `sigwaitinfo`, `siginfo_t.si_pid`, `struct rlimit`

- **udp/** curated entries temporarily removed (2026-09-21): `socket.c` UDP
  bind-ephemeral / getsockname / AF_UNSPEC unconnect from this PR regressed
  aarch64 interactive curl TLS and riscv64 dropbear SSH (`bad packet size`).
  Restored master `socket.c` to unblock full-boot; re-land UDP libc support
  in a follow-up with curl/SSH smokes held green.

## 2026-09-21 networking regression rollback

Libgloss/kernel additions for curated non-basic (CLOFORK, getifaddrs, limits
macros, userspace raise/sigaltstack, ppoll, getppid, UDP bind/getsockname,
sleep elapsed-time) regressed aarch64 `curl` TLS and riscv64 dropbear SSH
even after restoring master `socket.c`. Those libc/kernel files are restored
to master; curated `ci-nonbasic-100.tests` keeps suites that do not need them
(malloc/paths/stdio + remaining honest peers). Re-land support suite-by-suite
with curl+SSH held green on aarch64/riscv64 full boot.

