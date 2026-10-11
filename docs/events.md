# Event counters and timers

An event counter is what Linux calls an eventfd, a timer what it calls a
timerfd: a kernel object a program holds as an fd, reads for what
happened (a count, the expirations of a timer), polls for readiness, and
hands to a `poll` with its sockets and pipes. myos serves both from the
kernel (`kernel/src/evfd.rs`), as directories under `/dev` the Plan 9 way
(`kernel/src/fs/evfs.rs`), like the terminals (`docs/tty.md`) and the
`/net` conversations (`docs/sockets-curl.md`):

```
/dev/event/clone         read: allocates an event counter, answers its number N
/dev/event/N/data        the counter: 8-byte reads take the count, 8-byte writes add to it
/dev/event/N/ctl         commands: semaphore | counter | add N
/dev/event/N/status      count N / mode counter|semaphore
/dev/timer/clone         read: allocates a timer, answers its number N
/dev/timer/N/data        the timer: 8-byte reads take the expirations since the last
/dev/timer/N/ctl         commands: arm FIRST [INTERVAL] [abs] [realtime] | disarm
/dev/timer/N/status      armed LEFT INTERVAL | disarmed
/proc/self/fd/N          symlink: /dev/event/3/data for a counter, /dev/timer/0/data for a timer
```

A program reads `clone` once (the number comes as text, `3\n`), opens
`N/data` and keeps that fd: it is the eventfd or timerfd. The kernel knows
the fd as its own kind (`FdEntry::Event`, `FdEntry::Timer` in
`kernel/src/task/fd.rs`): `read`, `write` and `poll` on it are the kernel's,
a `fork` or `dup` shares the object (a reference count frees it with the
last fd), and `/proc/self/fd/N` names its directory, so a library holding
only the fd finds `ctl` and `status`. An object belongs to the user whose
process made it, as a `/net` conversation does: another user's processes
do not see it (`docs/security.md`; `/dev/event/**` and `/dev/timer/**`
are `dev.common`, every user's).

libgloss wraps the Linux calls over these files
(`toolchain/newlib/libgloss/myos/evfd.c`, `<sys/eventfd.h>`,
`<sys/timerfd.h>`): `eventfd`, `eventfd_read`, `eventfd_write`,
`timerfd_create`, `timerfd_settime`, `timerfd_gettime`. The Linux layer
maps `eventfd2` and the three `timerfd_*` syscalls onto the same objects
(`modules/linux/src/sys.rs`, `docs/linux-compat.md`), so a Linux program's
eventfd is a native fd and the layer keeps no state for it.

## Event counters

A counter starts at zero. A write of an 8-byte little-endian value adds it
to the count; a read takes the count, as 8 bytes, and leaves zero. A read
of an empty counter waits for a write (`EAGAIN` with `O_NONBLOCK`, which
libgloss and the Linux layer keep per fd, as for pipes); `poll` says
`POLLIN` while the count is not zero and `POLLOUT` while a write of one
would fit. A read or write of fewer than 8 bytes is refused, so is a
write of `0xffffffffffffffff` (`EINVAL` on Linux and in the Linux layer;
natively the generic failure, `EBADF` from libgloss, the native ABI having
no `EINVAL` result); a write that would carry the count past
`0xfffffffffffffffe` waits for a read (that one does not honour
`O_NONBLOCK`: the ceiling is not reached in practice).

The `ctl` commands:

| command | effect |
|---------|--------|
| `semaphore` | a read takes one from the count and answers 1 (`EFD_SEMAPHORE`) |
| `counter` | a read takes the whole count (the default) |
| `add N` | what a write of N to `data` does; `eventfd(initval, …)` uses it |

`status` reads `count 7\nmode counter\n`.

## Timers

A timer starts disarmed. `arm FIRST [INTERVAL] [abs] [realtime]` arms it,
in nanoseconds: `FIRST` from now, or a clock value with `abs` (the
monotonic clock, `CLOCK_MONOTONIC`; the wall clock with `realtime`, which
needs the RTC read at boot and is `EINVAL` without it); `INTERVAL`
between the expirations after the first (0, or left out: one expiry);
`FIRST` 0 disarms, as a zero `it_value` does on Linux. `disarm` stops it.
Arming forgets the expirations not read yet.

An expiry adds one to the timer's count of expirations (a periodic timer
that was not read for a while adds all the intervals missed). A read takes
the count, as 8 bytes, and waits while it is zero (`EAGAIN` with
`O_NONBLOCK`); `poll` says `POLLIN` while it is not. `status` reads `armed
LEFT INTERVAL` (nanoseconds until the next expiry, and between expiries)
or `disarmed`. The scheduler's tick (`kernel/src/task/sched.rs`,
`timer_tick`) fires the timers that are due and wakes their readers and
pollers; arming a timer makes it the CPU's next deadline when it is
nearer than the one it had, so an idle machine wakes for it
(`crate::task::deadline_soon`).

A timerfd's clock matters only to `TFD_TIMER_ABSTIME`: libgloss remembers
`CLOCK_REALTIME` per fd (forgotten by `exec`) and arms with `realtime`;
the Linux layer, where every clock is the wall clock
(`docs/linux-compat.md`), arms relative to now.

## From the shell

```
$ id=$(cat /dev/event/clone); echo add 2 > /dev/event/$id/ctl; cat /dev/event/$id/status
count 2
mode counter
$ id=$(cat /dev/timer/clone); echo arm 1000000000 500000000 > /dev/timer/$id/ctl; cat /dev/timer/$id/status
armed 999871000 500000000
```

An object made this way, with its `data` never opened, stays until a
program opens and closes it (as a `/net` conversation does).

## Tests

`user/c/evfd_smoke.c` (`t evfd` in `user/c/test.sh`) runs the counters and
timers through libgloss: the count, `EAGAIN`, `EINVAL`, the semaphore
mode, a read woken by a forked child's write, `status`, a one-shot and a
periodic timer, `gettime`, `disarm`, an absolute time.
`linux-compat/tests/linux-smoke.c` does the same from a musl program
through the Linux layer.
