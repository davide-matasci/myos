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
| **signal** | 32 | yes | raise/block/ignore/sigaltstack/ppoll/exec |
| **stdio** | 22 | yes | printf formatting |
| **namespace** | 159 | no | header pollution vs include-suite APIs (heavy tooling) |
| **pty** | 29 | no | pseudoterminals / controlling tty |
| **udp** | 207 | no | UDP socket semantics |
| **posix-parse** | 1 | no | parser helper |
| **include** | 3758 | no | API declaration corpus (not runtime tests) |

**Non-basic** = everything except `basic` (and the non-runtime `include` /
`posix-parse` helpers). Curated boot CI set: `misc/ci-nonbasic-100.tests`
(~100 paths, suite-prefixed). Wired the same way as basic: thin
`ci-smoke-copy.sh` staging + host prebuild + `make … TESTLIST=… report`.

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

