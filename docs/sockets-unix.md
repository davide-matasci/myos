# Local sockets: `/net/unix` and `AF_UNIX`

Local stream connections between processes, the way a display server talks
to its clients. As with TCP (`docs/sockets-curl.md`) there is no socket
syscall: the netfs module serves `/net/unix`, a Plan 9 style directory, and
the libgloss socket library maps `AF_UNIX` onto it. Unlike TCP, nothing goes
through netd: the bytes stay in the kernel.

## `/net/unix`

The files are those of `/net/tcp`, plus `listen`:

| File | |
|------|-|
| `clone` | reading it makes a conversation and returns its number `N` |
| `N/ctl` | commands: `announce NAME`, `connect NAME`, `pair`, `hangup` |
| `N/data` | the byte stream; `st_size` is the number of bytes waiting |
| `N/status` | `open`, `announced`, `connected` or `hangup` |
| `N/listen` | reading it hands out the next connection queued here (`"M\n"`, nothing when none waits); `st_size` is how many wait |

- `announce NAME` makes the conversation a listener under NAME. NAME is a
  name in `/net/unix` only: nothing is created in the filesystem, and the
  name is free again as soon as the listener closes. A second listener of
  the same name is refused.
- `connect NAME` connects to the listener of that name at once: the
  server's end is a new conversation, queued on the listener (up to 8)
  until it is taken from the listener's `listen`. No listener, or a full
  queue: the write fails.
- `pair` makes a second conversation connected to this one and queues it on
  this one's `listen` (`socketpair`).
- `hangup` ends the connection both ways (`shutdown`).

Each end buffers up to 8 KiB written by its peer. Reads and writes never
wait, as for TCP: an empty `data` reads 0 bytes (`status` turns `hangup`
when the peer is gone; what is buffered can still be read first), and a
write takes what fits and fails when nothing does. The last close of a
conversation's `data` ends it; a listener's queued, never-accepted
connections end with it (their clients see `hangup`). Up to 32
conversations exist at once.

The code is `modules/netfs/src/unix.rs`. A client and its server usually
run on different CPUs at the same time and module calls take no kernel
lock, so the unix conversations have their own spinlock.

## `AF_UNIX` in libgloss

`toolchain/newlib/libgloss/myos/socket.c`, with `<sys/un.h>`:

| Call | `/net/unix` |
|------|-------------|
| `socket(AF_UNIX, SOCK_STREAM, 0)` | `clone`, open `ctl` and `data`; returns the data fd |
| `bind(sockaddr_un)` | remembers the name (an abstract name, leading NUL, becomes `@name`) |
| `listen` | `announce NAME`; `EADDRINUSE` if the name has a listener |
| `accept` | takes a number from `listen` and opens its `data`; blocking waits in `poll` for a queued connection, nonblocking says `EAGAIN` |
| `connect(sockaddr_un)` | `connect NAME`; done at once, or `ECONNREFUSED` (no listener, or its queue is full) |
| `socketpair(AF_UNIX, SOCK_STREAM, 0, sv)` | `pair`, then the second end from `listen` |
| `read` / `recv` | `data`; empty and blocking waits for data or `hangup` (EOF) |
| `write` / `send` | `data`; a full peer buffer waits in `poll` for room (blocking) or says `EAGAIN`; a closed peer `EPIPE` (no `SIGPIPE`) |
| `poll` / `select` | netfs's `poll` hook: `POLLIN` for bytes, a queued connection or the peer gone, `POLLOUT` while the peer's buffer has room |
| `getsockname` / `getpeername` | the bound or connected name; accepted sockets have the listener's name and an unnamed peer |

`SOCK_DGRAM` is not supported (`EPROTONOSUPPORT`).

## Not yet

- Passing file descriptors (`SCM_RIGHTS`) and credentials (`SCM_CREDENTIALS`,
  `SO_PEERCRED`), and shared memory between processes: issue #283.
- The Linux layer (`modules/linux/src/net.rs`) maps `AF_INET` and, of
  `AF_UNIX`, only `socketpair`.

## Test

`user/c/unix_smoke.c` (`t unix` in `user/c/test.sh`, in the mini list):
`socketpair` both ways and EOF, `EADDRINUSE` and `ECONNREFUSED`, a
nonblocking `accept`, `poll` on a listener, a forked client exchanging a
greeting and then sending 64 KiB (more than the buffer, so its writes wait
for the reader), EOF after it closes, and the name free again after the
listener closes.
