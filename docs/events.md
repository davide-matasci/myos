# Event counters, timers, wait sets and signal fds

myos has four kinds of kernel event objects, each held as an fd:

| myos | Linux | A read gives |
|------|-------|--------------|
| event counter | eventfd | a count |
| timer | timerfd | the expirations of a timer |
| wait set | epoll instance | the members of a set that are ready |
| signal fd | signalfd | the signals that arrived |

A program reads such an fd for what happened, polls it for readiness, and
hands it to a `poll` with its sockets and pipes. The kernel serves all
four (`kernel/src/evfd.rs`) as directories under `/dev`, the Plan 9 way
(`kernel/src/fs/evfs.rs`), like the terminals (`docs/tty.md`) and the
`/net` conversations (`docs/net.md`):

```
/dev/event/clone         open: allocates an event counter, the fd is its N/data
/dev/event/N/data        the counter: 8-byte reads take the count, 8-byte writes add to it
/dev/event/N/ctl         commands: semaphore | counter | add N
/dev/event/N/status      count N / mode counter|semaphore
/dev/timer/clone         open: allocates a timer, the fd is its N/data
/dev/timer/N/data        the timer: 8-byte reads take the expirations since the last
/dev/timer/N/ctl         commands: arm FIRST [INTERVAL] [abs] [realtime] | disarm
/dev/timer/N/status      armed LEFT INTERVAL | disarmed
/dev/wait/clone          open: allocates a wait set, the fd is its N/data
/dev/wait/N/data         the set: 16-byte reads take the ready members {u64 cookie, u32 events, u32 0}
/dev/wait/N/ctl          commands: add FD EVENTS COOKIE | mod FD EVENTS COOKIE | del FD
/dev/wait/N/status       members N, then one FD EVENTS COOKIE line per member
/dev/sig/clone           open: allocates a signal fd, the fd is its N/data
/dev/sig/N/data          the fd: 16-byte reads take the reader's pending signals of the mask {u32 signo, u32 code, i32 pid, u32 uid}
/dev/sig/N/ctl           commands: mask N,N,... | mask none
/dev/sig/N/status        mask N,N,... | mask none
/proc/self/fd/N          symlink: /dev/event/3/data for a counter, /dev/timer/0/data, /dev/wait/1/data, /dev/sig/0/data
```

## Objects and fds

A program opens `clone` and keeps that fd: it is the new object's
`N/data`, the same pattern as `/dev/pts/clone` (`docs/tty.md`) and a
`/net` protocol's `clone` (`docs/net.md`).

- The object goes with the last fd on it, so none is ever left that
  nobody holds. A `fork` or `dup` shares it.
- The kernel knows the fd as its own kind (`kernel/src/task/fd.rs`), and
  `read`, `write` and `poll` on it are the kernel's.
- `/proc/self/fd/N` names the object's directory, so a library holding
  only the fd finds `ctl` and `status`.
- An object belongs to the user whose process made it; other users do not
  see it (`docs/security.md`). `/dev/event/**` and `/dev/timer/**` are
  `dev.common`.
- A read that would wait says `EAGAIN` with `O_NONBLOCK`; libgloss and
  the Linux layer keep that flag per fd, as for pipes.

The C library wraps the Linux calls over these files
(`toolchain/libmyos/evfd.c`, shared by libgloss and mlibc; headers
`<sys/eventfd.h>`, `<sys/timerfd.h>`, `<sys/epoll.h>`,
`<sys/signalfd.h>`). The Linux layer maps `eventfd2`, `timerfd_*`,
`epoll_*` and `signalfd*` onto the same objects (`modules/linux/src/sys.rs`,
`docs/linux-compat.md`), so a Linux program's event fds are native fds.

## Event counters

A write of an 8-byte little-endian value adds to the count; a read takes
the count and leaves zero, or waits while it is zero. `poll` says
`POLLIN` while the count is not zero, `POLLOUT` while a write of one
fits. A read or write of fewer than 8 bytes is refused, and so is a write
of `0xffffffffffffffff`: `EINVAL` on Linux and in the Linux layer, but
`EBADF` from libgloss, because the native ABI has no `EINVAL` result.

| ctl | effect |
|-----|--------|
| `semaphore` | a read takes one from the count and answers 1 (`EFD_SEMAPHORE`) |
| `counter` | a read takes the whole count (the default) |
| `add N` | a write of N; `eventfd(initval, …)` uses it |

## Timers

A timer starts disarmed. The `ctl` commands, in nanoseconds:

- **`arm FIRST [INTERVAL] [abs] [realtime]`**: `FIRST` is the time from
  now, or a clock value with `abs`, on the monotonic clock or the wall
  clock with `realtime` (which needs the RTC read at boot). `INTERVAL` 0
  or left out means one expiry; `FIRST` 0 disarms. Arming forgets the
  expirations not yet read.
- **`disarm`** stops it.

A read takes the expirations since the last, a periodic timer counting
every interval it missed. The scheduler's tick fires due timers
(`kernel/src/task/sched.rs`), and arming makes the timer the CPU's next
deadline when nearer, so an idle machine wakes for it. A timerfd's clock
matters only to `TFD_TIMER_ABSTIME`: libgloss remembers `CLOCK_REALTIME`
per fd (forgotten by `exec`), the Linux layer per timer.

## Wait sets

A wait set holds members: another fd's open object (a pipe end, a
socket's file, an event object, another set), asked for a readiness and
tagged with a cookie.

- A read gives the members ready now as 16-byte records `{u64 cookie, u32
  events, u32 0}`, `events` in `poll(2)` bits (`POLLIN` 1, `POLLOUT` 4,
  and always `POLLERR` 8, `POLLHUP` 0x10). With none ready it waits.
- `epoll_wait`'s timed wait is a `poll` on the set's fd, then a read.
  libgloss and the Linux layer keep the set's fd non-blocking, so a member
  that stopped being ready in between gives nothing rather than a wait.
- A set may be a member of another, but not in a cycle (Linux's `ELOOP`).
- A member is the open object, not the fd number: a `dup` or a fork's
  copy is the same member, and closing the object's last fd anywhere
  removes it from every set, as on Linux. The set is shared with a fork.
  A set holds 1024 members at most.

| ctl | effect |
|-----|--------|
| `add FD EVENTS COOKIE` | the object behind FD (an fd of the writer) joins |
| `mod FD EVENTS COOKIE` | new events and cookie, re-armed |
| `del FD` | leaves |

`EVENTS` is a comma list of `in`, `out`, `et` (edge-triggered), `oneshot`
(disabled after one report until a `mod`), or `none` (errors and hangups
only). `status` marks a fired `oneshot` member ` disabled`.

### Change counts

For `et`, every object a set can hold keeps a **change count** that moves
whenever its readiness may have changed (`entry_poll_changes` in
`kernel/src/task/fd.rs`). An edge-triggered member is reported again only
once its count moved, as Linux reports an fd again when its object woke
its waiters. A pipe's count moves on every write, as Linux wakes a polled
pipe's readers on every write. A module file's count comes from its
`poll` hook (`changes`, `modules/abi`); netfs counts per conversation. The
kernel reads the count before the readiness it goes with, so an edge
during a scan is never missed.

## Signal fds

A signal fd is a mask (`ctl`: `mask N,N,...` or `mask none`). A read
takes the lowest pending signals of the mask from the *reading* process
instead of delivering them, one 16-byte record `{u32 signo, u32 code, i32
pid, u32 uid}` each. `code`, `pid` and `uid` are 0: the kernel keeps no
sender.

- With nothing pending a read waits, and ends with `EINTR` when a signal
  that acts comes first, like `sigwait`.
- As on Linux, the reader should block the signals, or they are delivered
  before a read gets them.
- The fd carries no process: a fork's child reads its own signals through
  it.

libgloss's `struct signalfd_siginfo` is this 16-byte record with Linux's
field names; the Linux layer expands it into Linux's 128-byte struct.

## From the shell

A shell holds an object with an fd on its `clone`, and finds its directory
through the fd's link:

```
$ exec 3<> /dev/event/clone; d=$(readlink /proc/self/fd/3); d=${d%/data}
$ echo add 2 > $d/ctl; cat $d/status
count 2
mode counter
$ exec 3>&-
```

`cat /dev/event/clone` makes a counter and waits for a count, as a read
of an eventfd does.

## Tests

`user/c/evfd_smoke.c`, `epoll_smoke.c` and `sigfd_smoke.c` (`evfd`,
`epoll`, `sigfd` in `user/c/test.sh`) cover the four kinds through
libgloss; `linux-compat/tests/linux-smoke.c` does the same through the
Linux layer, once more in a chroot (`linux_evfd_root`). End to end,
`alpine_python_asyncio` (`user/tests/kernel.sh`) runs Python's asyncio,
whose selector is a `/dev/wait` set.

## History

- **Per-object change counts** (issue #456). An `et` member used to be
  reported again after any wake in the system, which a busy system did
  between two `epoll_wait` calls.
