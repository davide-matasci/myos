# Local sockets: `/net/unix` and `AF_UNIX`

Local sockets connect processes on the same machine. Stream connections
are how a display server talks to its clients; record connections
(`SOCK_SEQPACKET`) are how WebKit talks to its web process. As with TCP
(`docs/net.md`) there is no socket syscall. The netfs module serves
`/net/unix`, a Plan 9 style directory, and the socket library maps
`AF_UNIX` onto it. Unlike TCP, nothing goes through netd: the bytes stay
in the kernel.

## `/net/unix`

The files are those of `/net/tcp`, plus `listen`, `rights` and `from`:

| File | |
|------|-|
| `clone` | opening it makes a conversation, and the fd is its `N/data` (whose `/proc/self/fd` link names it); its files are the opening process's user's alone (`docs/security.md`) |
| `N/ctl` | commands: `seqpacket`, `announce NAME`, `connect NAME`, `pair`, `hangup`, `rights FD...`, `norights`; reading it says `seqpacket` for a record conversation (`seqpacket cut` after a read that cut a record short), nothing for a stream |
| `N/data` | the byte stream, or the records; `st_size` is the number of bytes waiting (a record conversation's: the length of the next record) |
| `N/status` | `open`, `announced`, `connected` or `hangup` |
| `N/listen` | reading it hands out the next connection queued here (`"M\n"`, nothing when none waits); `st_size` is how many wait |
| `N/rights` | reading it makes the fds that came with the last read of `data` the reader's, and gives their numbers (`"5 6\n"`) |
| `N/from` | the pid of the process whose write the last read of `data` returned bytes of (`"12\n"`; nothing before a read) |

### ctl commands

- **`announce NAME`** makes the conversation a listener under NAME. NAME
  is a name in `/net/unix` only: nothing is created in the filesystem,
  and the name is free again as soon as the listener closes. A second
  listener of the same name is refused.
- **`connect NAME`** connects to the listener of that name at once. The
  server's end is a new conversation, queued on the listener (up to 8)
  until it is taken from the listener's `listen`. With no listener, or a
  full queue, the write fails.
- **`pair`** makes a second conversation connected to this one and queues
  it on this one's `listen` (`socketpair`).
- **`hangup`** ends the connection both ways (`shutdown`).
- **`seqpacket`**, written before `announce`, `connect` or `pair`, makes
  the conversation keep records (see "Records").
- **`rights FD...`** passes the writer's own fds with the next write of
  `data`; **`norights`** takes that back (see "Passing fds").

### Records

A record conversation (`seqpacket`) keeps message boundaries:

- Each write is one record, taken whole or refused. There is no partial
  write.
- A read returns at most one record, and drops the part of it the reader
  had no room for.
- A record is at most 8 KiB. The kernel hands a file a larger write in
  pieces, each of which would become a record of its own, so the
  libraries refuse such a write (`EMSGSIZE`) before it is written (issue
  #645). This holds for `writev` too (never one record per buffer), and
  for a socket the process inherited or received.
- A record that does not fit the room left in the 64 KiB buffer, or past
  the 512 records a buffer holds (each costs bookkeeping beyond its
  bytes), is refused until it fits. `poll` says `POLLOUT` only once it
  would.
- `pair`'s other end and the server's end of a `connect` keep records too.
  A `connect` from a stream conversation to a record listener, or the
  other way round, is refused.

### Passing fds

`rights FD...` passes the writer's own fds (`SCM_RIGHTS`) with the next
write of `data`.

- The fds are held as the writer's fds hold them: a pipe end, an open
  file description with its rights, a socket. This is the kernel's
  `fd_hold`, a deliberate grant under the policy (`docs/security.md`).
- They come with that write's first byte. A read of `data` never goes past
  the bytes before them, and the read that starts there takes them.
- Reading `rights` then installs them as the reader's lowest free fds
  (`fd_install`).
- The next read of `data` closes the ones nobody took, as Linux's `read`
  of a socket does. The end of the conversation does the same for every
  fd it still carries.
- `norights` lets go of what `rights` took when the write it was for
  failed. A retry sends them again.
- At most 253 fds go with one write.
- A socket passed over itself, or in a cycle of sockets nobody holds,
  stays open: there is no collector.

### Sender pid

Each write is recorded with its writer's pid, and a read of `data` never
returns the bytes of two writers. `from` then names the one it returned.
This serves `SCM_CREDENTIALS` and the Linux layer's `SO_PASSCRED`; WebKit
learns its web process's pid that way.

### Buffers and lifetime

- Each end buffers up to 64 KiB written by its peer. The buffer is
  allocated as it fills and let go of once read.
- Reads and writes never wait, as for TCP. An empty `data` reads 0 bytes;
  `status` turns `hangup` when the peer is gone, and what is buffered can
  still be read first. A write takes what fits and fails when nothing
  does.
- The last close of a conversation's `data` ends it. A listener's queued,
  never-accepted connections end with it, and their clients see `hangup`.
- There is no fixed number of conversations. Each holds an fd, so the fd
  limits bound them. With the kernel heap full, `clone` (or a write) is
  refused.

The code is `modules/netfs/src/unix.rs`. A client and its server usually
run on different CPUs at the same time, and module calls take no kernel
lock, so the unix conversations have their own spinlock.

## `AF_UNIX` in the socket library

`toolchain/libmyos/socket.c` (shared by libgloss and mlibc), with
`<sys/un.h>`:

| Call | `/net/unix` |
|------|-------------|
| `socket(AF_UNIX, SOCK_STREAM, 0)` | opens `clone` (the data fd) and `ctl`; returns the data fd |
| `socket(AF_UNIX, SOCK_SEQPACKET, 0)` | the same, then `seqpacket` |
| `bind(sockaddr_un)` | remembers the name (an abstract name, leading NUL, becomes `@name`) |
| `listen` | `announce NAME`; `EADDRINUSE` if the name has a listener |
| `accept` | takes a number from `listen` and opens its `data`; blocking waits in `poll` for a queued connection, nonblocking says `EAGAIN` |
| `connect(sockaddr_un)` | `connect NAME`; done at once, or `ECONNREFUSED` (no listener, or its queue is full) |
| `socketpair(AF_UNIX, SOCK_STREAM or SOCK_SEQPACKET, 0, sv)` | `pair`, then the second end from `listen` |
| `read` / `recv` | `data`; empty and blocking waits for data or `hangup` (EOF) |
| `write` / `send` | `data`; a full peer buffer waits in `poll` for room (blocking) or says `EAGAIN`; a closed peer `EPIPE` (no `SIGPIPE`) |
| `poll` / `select` | netfs's `poll` hook: `POLLIN` for bytes, a queued connection or the peer gone, `POLLOUT` while the peer's buffer has room |
| `getsockname` / `getpeername` | the bound or connected name; accepted sockets have the listener's name and an unnamed peer |
| `sendmsg` | `rights` with an `SCM_RIGHTS` message, then the data gathered into one write (`norights` when it fails): one record on a record socket, `EMSGSIZE` past 8 KiB (`write` and `writev` too) |
| `recvmsg` | one read of `data`, then `rights` for the fds that came with it as an `SCM_RIGHTS` message; those `msg_control` has no room for are closed (`MSG_CTRUNC`); `MSG_CMSG_CLOEXEC`; on a record socket whose read filled the buffers, `ctl` says whether the record was longer (`MSG_TRUNC`) |
| a socket fd inherited by `exec` or received in `SCM_RIGHTS` | known by its `/proc/self/fd` link (`/net/unix/N/data`) at its first socket call: a stream or a record socket as `ctl` says, connected or listening as `status` says |

`SOCK_DGRAM` is not supported (`EPROTONOSUPPORT`).

The Linux layer maps a Linux program's `AF_UNIX` calls onto `/net/unix`
the same way (`modules/linux/src/net.rs`, `docs/linux-compat.md`), so
Linux and native programs connect to each other by name.

## Limits

- No credentials in the socket library, and no true `SO_PEERCRED`: the
  Linux layer's gives the caller's own pid (issue #340). The sender's pid
  of what a read returns is there (`from`, above).
- File descriptors do pass (`SCM_RIGHTS`, above), and processes can share
  memory (`shm_open`, `memfd_create`, System V `shmget`, `MAP_SHARED`:
  README).

## Tests

`user/c/unix_smoke.c` runs as `t unix` in `user/c/test.sh` (in the mini
list), and its record sockets as `t unix_seqpacket` (`unix_smoke
seqpacket`). It covers:

- `socketpair` both ways, and EOF;
- 60000 bytes written before the reader reads any;
- 48 conversations open at once;
- `EADDRINUSE` and `ECONNREFUSED`;
- a nonblocking `accept`, and `poll` on a listener;
- a forked client exchanging a greeting and then sending 256 KiB (more
  than the buffer, so its writes wait for the reader), and EOF after it
  closes;
- the name free again after the listener closes.

For the Linux layer:

- `linux-smoke` (`t linux_smoke`) makes the same calls between Linux
  processes, an abstract name too;
- `t linux_unix` (`user/tests/kernel.sh`) connects a Linux client to a
  native server (`unix_smoke echo NAME`);
- the full list runs Alpine's `xdpyinfo` against the native X server
  (`t alpine_x11`).
