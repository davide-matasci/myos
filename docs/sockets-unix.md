# Local sockets: `/net/unix` and `AF_UNIX`

Local sockets connect processes on the same machine. Stream connections
are how a display server talks to its clients; record connections
(`SOCK_SEQPACKET`) are how WebKit talks to its web process. As with TCP
(`docs/net.md`) there is no socket syscall. The netfs module serves
`/net/unix`, a Plan 9 style directory, and the socket library maps
`AF_UNIX` onto it. Unlike TCP, nothing goes through netd: the bytes stay
in the kernel (`modules/netfs/src/unix.rs`).

## `/net/unix`

The files are those of `/net/tcp`, plus `listen`, `rights` and `from`:

| File | |
|------|-|
| `clone` | opening it makes a conversation; the fd is its `N/data`. Its files are the opening user's alone (`docs/security.md`) |
| `N/ctl` | the commands below; a read says `seqpacket` for a record conversation (`seqpacket cut` after a read cut a record short) |
| `N/data` | the byte stream or the records; `st_size` is the bytes waiting (or the next record's length) |
| `N/status` | `open`, `announced`, `connected` or `hangup` |
| `N/listen` | a read hands out the next queued connection's number (`"M\n"`); `st_size` is how many wait |
| `N/rights` | a read installs the fds that came with the last read of `data` and gives their numbers (`"5 6\n"`) |
| `N/from` | the pid whose write the last read of `data` returned (`"12\n"`) |

The `ctl` commands:

- **`announce NAME`** makes a listener under NAME. NAME lives in
  `/net/unix` only: nothing is created in the filesystem, and the name is
  free again once the listener closes.
- **`connect NAME`** connects at once. The server's end is a new
  conversation queued on the listener (up to 8) until taken from its
  `listen`. With no listener or a full queue the write fails.
- **`pair`** makes a second conversation connected to this one and queues
  it on this one's `listen` (`socketpair`).
- **`hangup`** ends the connection both ways (`shutdown`).
- **`seqpacket`**, written before `announce`, `connect` or `pair`, makes
  the conversation keep records.
- **`rights FD...`** passes the writer's fds with the next write of
  `data`; **`norights`** takes that back when the write failed.

## Behavior

- **Buffers.** Each end buffers up to 64 KiB from its peer, allocated as
  it fills. Reads and writes in the kernel never wait: an empty `data`
  reads 0 bytes and `status` turns `hangup` once the peer is gone (what is
  buffered is still read first). A write takes what fits.
- **Records.** Each write is one record, taken whole or refused, and a
  read returns at most one, dropping what did not fit. A record is at most
  8 KiB: the kernel hands a file a larger write in pieces, so the
  libraries refuse it with `EMSGSIZE`, `writev` included (issue #645). A
  buffer holds at most 512 records. Stream and record ends cannot connect
  to each other.
- **Passing fds** (`SCM_RIGHTS`). The fds are held as the writer's fds
  hold them (`fd_hold`, a deliberate grant under the policy,
  `docs/security.md`). They come with that write's first byte: a read
  never goes past the bytes before them, and the read starting there takes
  them. The next read of `data` closes the ones nobody took, as Linux
  does. At most 253 go with one write.
- **Sender pid.** A read of `data` never returns the bytes of two writers,
  and `from` names the writer. This serves `SCM_CREDENTIALS` and the
  Linux layer's `SO_PASSCRED`; WebKit learns its web process's pid this
  way.
- **Lifetime.** The last close of `data` ends a conversation, and a
  listener's unaccepted connections with it. There is no fixed number of
  conversations: the fd limits and the kernel heap bound them.

## `AF_UNIX` in the socket library

`toolchain/libmyos/socket.c` (shared by libgloss and mlibc), with
`<sys/un.h>`:

| Call | `/net/unix` |
|------|-------------|
| `socket(AF_UNIX, SOCK_STREAM \| SOCK_SEQPACKET, 0)` | opens `clone` (and writes `seqpacket` for records) |
| `bind` | remembers the name; an abstract name (leading NUL) becomes `@name` |
| `listen` / `accept` | `announce NAME` (`EADDRINUSE` if taken); a number from `listen`, then its `data` |
| `connect` | `connect NAME`; done at once, or `ECONNREFUSED` |
| `socketpair` | `pair`, then the second end from `listen` |
| `read` / `write` | `data`; a blocking call waits in `poll`; a closed peer is EOF or `EPIPE` (no `SIGPIPE`) |
| `sendmsg` / `recvmsg` | `rights` with an `SCM_RIGHTS` message; fds `msg_control` has no room for are closed (`MSG_CTRUNC`); `MSG_TRUNC` from `ctl` |
| `getsockname` / `getpeername` | the bound or connected name; an accepted socket has the listener's name |

A socket fd inherited by exec or received by `SCM_RIGHTS` is known by its
`/proc/self/fd` link at its first socket call. `SOCK_DGRAM` is not
supported (`EPROTONOSUPPORT`).

The Linux layer maps Linux `AF_UNIX` calls onto `/net/unix` the same way
(`modules/linux/src/net.rs`, `docs/linux-compat.md`), so Linux and native
programs connect to each other by name.

## Limits

- No true `SO_PEERCRED`: the Linux layer's gives the caller's own pid
  (issue #340). The sender's pid is there (`from`).
- A socket passed over itself, or in a cycle of sockets nobody holds,
  stays open: there is no collector.

## Tests

`user/c/unix_smoke.c` runs as `unix` and `unix_seqpacket` in
`user/c/test.sh` (tag `netfs`). For the Linux layer, `linux_smoke` makes
the same calls between Linux processes, `linux_unix`
(`user/tests/kernel.sh`) connects a Linux client to a native server, and
`alpine_x11` runs Alpine's `xdpyinfo` against the native X server.
