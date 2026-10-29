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

A program opens `clone` and keeps that fd. The fd is the new object's
`N/data`: the eventfd, timerfd, epoll fd or signalfd. This is the same
pattern as opening `/dev/pts/clone`, which gives a new pair's master
(`docs/tty.md`), or a `/net` protocol's `clone`, which gives a new
conversation's `data` (`docs/net.md`).

- The object belongs to its fds: it goes with the last fd on it, so no
  object is ever left that nobody holds.
- The kernel knows the fd as its own kind (`FdEntry::Event`,
  `FdEntry::Timer`, `FdEntry::Wait`, `FdEntry::Sig` in
  `kernel/src/task/fd.rs`). `read`, `write` and `poll` on it are the
  kernel's.
- A `fork` or `dup` shares the object. A reference count frees it with
  the last fd.
- `/proc/self/fd/N` names the object's directory, so a library holding
  only the fd finds `ctl` and `status`.
- An object belongs to the user whose process made it, as a `/net`
  conversation does. Another user's processes do not see it
  (`docs/security.md`). `/dev/event/**` and `/dev/timer/**` are
  `dev.common`, every user's.

### C library and Linux layer

The C library wraps the Linux calls over these files
(`toolchain/libmyos/evfd.c`, shared by libgloss and mlibc; libgloss's
headers `<sys/eventfd.h>`, `<sys/timerfd.h>`, `<sys/epoll.h>`,
`<sys/signalfd.h>`): `eventfd`, `eventfd_read`, `eventfd_write`,
`timerfd_create`, `timerfd_settime`, `timerfd_gettime`, `epoll_create`,
`epoll_create1`, `epoll_ctl`, `epoll_wait`, `epoll_pwait`, `signalfd`.

The Linux layer maps `eventfd2`, the `timerfd_*`, the `epoll_*` and the
`signalfd*` syscalls onto the same objects (`modules/linux/src/sys.rs`,
`docs/linux-compat.md`). A Linux program's eventfd, epoll fd or signalfd
is therefore a native fd, and the layer keeps no state for it beyond a
flag.

## Event counters

A counter starts at zero.

- A write of an 8-byte little-endian value adds it to the count.
- A read takes the count, as 8 bytes, and leaves zero.
- A read of an empty counter waits for a write. With `O_NONBLOCK` it
  says `EAGAIN`; libgloss and the Linux layer keep that flag per fd, as
  for pipes.
- `poll` says `POLLIN` while the count is not zero, and `POLLOUT` while a
  write of one would fit.
- A read or write of fewer than 8 bytes is refused, and so is a write of
  `0xffffffffffffffff`. That is `EINVAL` on Linux and in the Linux layer.
  Natively it is the generic failure, `EBADF` from libgloss, because the
  native ABI has no `EINVAL` result.
- A write that would carry the count past `0xfffffffffffffffe` waits for a
  read. That wait ignores `O_NONBLOCK`; the ceiling is not reached in
  practice.

The `ctl` commands:

| command | effect |
|---------|--------|
| `semaphore` | a read takes one from the count and answers 1 (`EFD_SEMAPHORE`) |
| `counter` | a read takes the whole count (the default) |
| `add N` | what a write of N to `data` does; `eventfd(initval, …)` uses it |

`status` reads `count 7\nmode counter\n`.

## Timers

A timer starts disarmed. The `ctl` commands, in nanoseconds:

- **`arm FIRST [INTERVAL] [abs] [realtime]`** arms it.
  - `FIRST` is the time from now, or a clock value with `abs`. The clock
    is the monotonic one (`CLOCK_MONOTONIC`), or the wall clock with
    `realtime`. `realtime` needs the RTC read at boot and is `EINVAL`
    without it.
  - `INTERVAL` is the time between the expirations after the first. 0, or
    left out, means one expiry.
  - `FIRST` 0 disarms, as a zero `it_value` does on Linux.
  - Arming forgets the expirations not read yet.
- **`disarm`** stops it.

Each expiry adds one to the timer's count of expirations. A periodic timer
that was not read for a while adds all the intervals it missed.

- A read takes the count, as 8 bytes, and waits while it is zero (`EAGAIN`
  with `O_NONBLOCK`).
- `poll` says `POLLIN` while the count is not zero.
- `status` reads `armed LEFT INTERVAL` (nanoseconds until the next expiry,
  and between expiries) or `disarmed`.

The scheduler's tick (`kernel/src/task/sched.rs`, `timer_tick`) fires the
timers that are due and wakes their readers and pollers. Arming a timer
makes it the CPU's next deadline when it is nearer than the one it had, so
an idle machine wakes for it (`crate::task::deadline_soon`).

A timerfd's clock matters only to `TFD_TIMER_ABSTIME`. libgloss remembers
`CLOCK_REALTIME` per fd (forgotten by `exec`) and arms with `realtime`;
the Linux layer remembers it per timer (`docs/linux-compat.md`).

## Wait sets

A wait set holds members. A member is another fd's open object (a pipe
end, a socket's file, an event counter, a timer, another set), tagged with
the fd it was added by and a cookie, and asked for a readiness.

- A read of the set gives the members ready now, as many as the buffer
  holds, as 16-byte records `{u64 cookie, u32 events, u32 0}`. `events` is
  the `poll(2)` bits that hold: `POLLIN` 1, `POLLOUT` 4, `POLLERR` 8,
  `POLLHUP` 0x10. The last two are always reported.
- A read with no member ready waits for one (`EAGAIN` with `O_NONBLOCK`).
- `poll` says `POLLIN` while a member is ready.
- A timed wait, what `epoll_wait` does, is a `poll` on the set's fd with
  the timeout, then a read. libgloss and the Linux layer keep the set's fd
  non-blocking, so a member that stopped being ready in between gives
  nothing rather than a wait.
- A set may be a member of another, but not of itself, and not in a
  cycle (refused, as Linux's `ELOOP`).

The `ctl` commands name the member by an fd of the writing process:

| command | effect |
|---------|--------|
| `add FD EVENTS COOKIE` | the object behind FD joins, asked for EVENTS; refused when the set already has it by that fd |
| `mod FD EVENTS COOKIE` | new events and cookie, re-armed (a `oneshot` member fires again) |
| `del FD` | leaves |

`EVENTS` is a comma list of:

- `in`, `out`;
- `et`: edge-triggered. A readiness is reported once, and again only once
  the member's object changed since, as Linux reports an fd again when its
  object woke its waiters;
- `oneshot`: reported once, then disabled until a `mod`;
- or `none`: errors and hangups only.

`status` reads `members N` and one `FD EVENTS COOKIE` line per member,
with ` disabled` after a fired `oneshot`.

A member is the open object, not the fd number. A `dup` of the fd, or the
same fd in a forked child, refers to the same member. Closing the
object's last fd (in every process) takes it out of every set, as on
Linux (`fd_drop` in `kernel/src/task/fd.rs`). The set itself is shared
with a fork, like Linux's: the child's `add` shows in the parent. A set
holds 1024 members at most.

### Change counts

Every object a set can hold keeps a **change count** for `et`
(`entry_poll_changes` in `kernel/src/task/fd.rs`): a number that moves
whenever its readiness may have changed.

| Object | Its count moves with |
|--------|----------------------|
| pipe | every write (new data is an edge, as on Linux, which wakes a polled pipe's readers on every write); a read that made room in a full pipe; an end closed, either end's (Linux wakes an epoll on a pipe from both of its wait queues too) |
| pty | its bytes, either way |
| event counter | its count |
| timer | an expiry or an arming |
| signal fd | its mask, and the reading process's pending signals |
| wait set | its `ctl` and its members' counts |
| console | its input |
| module file (a socket's `/net` data, the raw keyboard, a network device's `data`) | what the module reports through its `poll` hook (`changes` in `ModuleVfsOps::poll` and `ModuleChrOps::poll`, `modules/abi`). netfs counts per conversation: bytes in or out, netd's replies, a hangup, the peer's state for a unix socket |

The kernel reads the count before the readiness it goes with. A change
during a scan therefore shows as a move at the next scan, and an edge is
never missed.

## Signal fds

A signal fd is a mask of signals. Its `ctl` takes `mask N,N,...` (by
number) or `mask none`, and `status` reads the mask back.

- A read takes the lowest pending signal of the mask from the *reading*
  process, instead of delivering it. It gives one 16-byte record `{u32
  signo, u32 code, i32 pid, u32 uid}` per signal, as many as fit. `code`,
  `pid` and `uid` are 0: the kernel keeps no sender.
- With no signal pending, a read waits for one (`EAGAIN` with
  `O_NONBLOCK`). It ends with `EINTR` when a signal that acts comes first
  (one outside the mask, or an unblocked one), like `sigwait`.
- `poll` says `POLLIN` while a signal of the mask is pending.
- As on Linux, the reader should block the signals: an unblocked one is
  delivered before a read gets to it.
- The fd carries no process: a fork's child reads its own signals through
  it.

A signal arriving at a process wakes its `sigwait`s, signal fd readers and
pollers (`send_locked` in `kernel/src/task/signals.rs`).

libgloss's `<sys/signalfd.h>` declares `struct signalfd_siginfo` as this
16-byte record, with Linux's names for its fields. The Linux layer expands
each record into Linux's 128-byte struct (`ssi_signo` is Linux's number,
the rest 0).

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

`cat /dev/event/clone` makes a counter and reads it, so it waits for a
count, as a read of an eventfd does.

## Tests

- `user/c/evfd_smoke.c` (`t evfd` in `user/c/test.sh`) runs the counters
  and timers through libgloss: the count, `EAGAIN`, `EINVAL`, the
  semaphore mode, a read woken by a forked child's write, `status`, a
  one-shot and a periodic timer, `gettime`, `disarm`, an absolute time.
- `user/c/epoll_smoke.c` (`t epoll`) runs the wait sets: pipes and an
  event counter as members, a timeout, a child's write ending a wait, `et`
  (once; again for new data; not for another pipe's traffic; on a unix
  socket too), `oneshot`, the errors, a member gone with its fd, a set in
  a set, the set shared with a fork.
- `user/c/sigfd_smoke.c` (`t sigfd`) runs the signal fds: a blocked signal
  read instead of delivered, one outside the mask delivered, `EAGAIN`, a
  child's kill ending a read, the mask changed, the fd in a wait set.
- `linux-compat/tests/linux-smoke.c` does the same from a musl program
  through the Linux layer. It runs once more chrooted into a bare root
  (`linux_evfd_root` in `user/tests/kernel.sh`): `linux --root` binds the
  system's `/dev` into the root, and the fds must be the same objects
  there.
- End to end, the full list runs an asyncio script in the Alpine Python
  root (`alpine_python_asyncio` in `user/tests/kernel.sh`,
  `docs/linux-compat.md`). Python's event loop selects on `select.epoll`,
  a `/dev/wait` set, with its signal wakeup fd and its sockets as members.

## History

- **Per-object change counts** (issue #456). An `et` member used to be
  reported again after any wake in the system, which a busy system did
  between two `epoll_wait` calls.
