# Event counters, timers, wait sets and signal fds

An event counter is what Linux calls an eventfd, a timer what it calls a
timerfd, a wait set what it calls an epoll instance, a signal fd what it
calls a signalfd: a kernel object a program holds as an fd, reads for
what happened (a count, the expirations of a timer, the members of a set
that are ready, the signals that arrived), polls for readiness, and hands
to a `poll` with its sockets and pipes. myos serves all four from the
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
/dev/wait/clone          read: allocates a wait set, answers its number N
/dev/wait/N/data         the set: 16-byte reads take the ready members {u64 cookie, u32 events, u32 0}
/dev/wait/N/ctl          commands: add FD EVENTS COOKIE | mod FD EVENTS COOKIE | del FD
/dev/wait/N/status       members N, then one FD EVENTS COOKIE line per member
/dev/sig/clone           read: allocates a signal fd, answers its number N
/dev/sig/N/data          the fd: 16-byte reads take the reader's pending signals of the mask {u32 signo, u32 code, i32 pid, u32 uid}
/dev/sig/N/ctl           commands: mask N,N,... | mask none
/dev/sig/N/status        mask N,N,... | mask none
/proc/self/fd/N          symlink: /dev/event/3/data for a counter, /dev/timer/0/data, /dev/wait/1/data, /dev/sig/0/data
```

A program reads `clone` once (the number comes as text, `3\n`), opens
`N/data` and keeps that fd: it is the eventfd, timerfd, epoll fd or
signalfd. The kernel knows the fd as its own kind (`FdEntry::Event`,
`FdEntry::Timer`, `FdEntry::Wait`, `FdEntry::Sig` in
`kernel/src/task/fd.rs`): `read`, `write` and `poll` on it are the
kernel's,
a `fork` or `dup` shares the object (a reference count frees it with the
last fd), and `/proc/self/fd/N` names its directory, so a library holding
only the fd finds `ctl` and `status`. An object belongs to the user whose
process made it, as a `/net` conversation does: another user's processes
do not see it (`docs/security.md`; `/dev/event/**` and `/dev/timer/**`
are `dev.common`, every user's).

libgloss wraps the Linux calls over these files
(`toolchain/newlib/libgloss/myos/evfd.c`, `<sys/eventfd.h>`,
`<sys/timerfd.h>`, `<sys/epoll.h>`, `<sys/signalfd.h>`): `eventfd`,
`eventfd_read`, `eventfd_write`, `timerfd_create`, `timerfd_settime`,
`timerfd_gettime`, `epoll_create`, `epoll_create1`, `epoll_ctl`,
`epoll_wait`, `epoll_pwait`, `signalfd`. The Linux layer maps `eventfd2`,
the `timerfd_*`, the `epoll_*` and the `signalfd*` syscalls onto the same
objects (`modules/linux/src/sys.rs`, `docs/linux-compat.md`), so a Linux
program's eventfd, epoll fd or signalfd is a native fd and the layer
keeps no state for it beyond a flag.

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

## Wait sets

A wait set holds members: other fds' open objects (a pipe end, a socket's
file, an event counter, a timer, another set), each tagged with the fd it
was added by and a cookie, and asked for a readiness. A read of the set
gives the members ready now, up to what the buffer holds, as 16-byte
records `{u64 cookie, u32 events, u32 0}`, `events` the `poll(2)` bits
that hold (`POLLIN` 1, `POLLOUT` 4, `POLLERR` 8, `POLLHUP` 0x10, the last
two always reported); a read with no member ready waits for one (`EAGAIN`
with `O_NONBLOCK`), and `poll` says `POLLIN` while one is. A timed wait is
a `poll` on the set's fd with the timeout, then a read: what `epoll_wait`
does (libgloss and the Linux layer keep the set's fd non-blocking, so a
member that stopped being ready in between gives nothing rather than a
wait). A set may be a member of another, not of itself, and not in a
cycle (refused, as Linux's `ELOOP`).

The `ctl` commands name the member by an fd of the writing process:

| command | effect |
|---------|--------|
| `add FD EVENTS COOKIE` | the object behind FD joins, asked for EVENTS; refused when the set already has it by that fd |
| `mod FD EVENTS COOKIE` | new events and cookie, re-armed (a `oneshot` member fires again) |
| `del FD` | leaves |

`EVENTS` is a comma list of `in`, `out`, `et` (edge-triggered: a readiness
is reported once, and again only once something happened since, any wake
in the system; Linux reports every new event and nothing else, so a busy
myos reports a few more, which an edge-triggered reader takes as a read
that finds nothing), `oneshot`
(reported once, then disabled until a `mod`), or `none` (errors and
hangups only). `status` reads `members N` and one `FD EVENTS COOKIE` line
per member, ` disabled` after a fired `oneshot`.

A member is the open object, not the fd number: a `dup` of the fd or the
same fd in a forked child refer to the same member, and closing the
object's last fd (in every process) takes it out of every set, as on
Linux (`fd_drop` in `kernel/src/task/fd.rs`). The set itself is shared
with a fork, like Linux's: the child's `add` shows in the parent. A set
holds 1024 members at most.

## Signal fds

A signal fd is a mask of signals (`ctl`: `mask N,N,...` by number, or
`mask none`; `status` reads it back). A read takes the lowest pending
signal of the mask from the *reading* process, one 16-byte record `{u32
signo, u32 code, i32 pid, u32 uid}` per signal (as many as fit; `code`,
`pid` and `uid` are 0: the kernel keeps no sender), instead of delivering
it; with none pending it waits for one (`EAGAIN` with `O_NONBLOCK`), and
ends with `EINTR` when a signal that acts (one outside the mask, or an
unblocked one) comes first, like `sigwait`. `poll` says `POLLIN` while
one is pending. As on Linux the signals should be blocked by the reader:
an unblocked one is delivered before a read gets to it. The fd carries no
process: a fork's child reads its own signals through it; a signal
arriving at a process wakes its `sigwait`s, signal fd readers and pollers
(`send_locked` in `kernel/src/task/signals.rs`).

libgloss's `<sys/signalfd.h>` declares `struct signalfd_siginfo` as this
16-byte record, with Linux's names for its fields; the Linux layer
expands each record into Linux's 128-byte struct (`ssi_signo` Linux's
number, the rest 0).

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
`user/c/epoll_smoke.c` (`t epoll`) runs the wait sets: pipes and an event
counter as members, a timeout, a child's write ending a wait, `et`,
`oneshot`, the errors, a member gone with its fd, a set in a set, the set
shared with a fork. `user/c/sigfd_smoke.c` (`t sigfd`) runs the signal
fds: a blocked signal read instead of delivered, one outside the mask
delivered, `EAGAIN`, a child's kill ending a read, the mask changed, the
fd in a wait set. `linux-compat/tests/linux-smoke.c` does the same from a
musl program through the Linux layer.
