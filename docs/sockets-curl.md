# Userspace BSD sockets + curl

## Design

myos has **no `socket()` syscall** and no kernel socket table. Networking is:

1. Kernel: virtio-net → `/dev/net0/data` + netfs Plan 9 `/net` + `/dev/netd/data` channel
2. Userspace `netd`: smoltcp over `/dev/net0/data`, its address, mask and
   default gateway from DHCP. The gateway may lie outside the address's
   prefix (Hetzner Cloud gives a /32 and the gateway 172.31.1.1): smoltcp
   carries a myos patch for that, accepting ARP from a router of a route
   (`user/net/smoltcp/`, applied into `target/smoltcp-myos` before netd is
   built, `PORT_PREPARE`). netd hands the lease to netfs, which shows it
   as `/net/ndb`, Plan 9 style:
   ```
   ip=10.0.2.15 ipmask=255.255.255.0 ipgw=10.0.2.2
   	dns=10.0.2.4
   ```
   One `dns=` line per DNS server the lease names; the file is empty until
   DHCP is done (and after the lease is lost). netd has a second smoltcp
   interface, `lo`, on a loopback device: it carries 127.0.0.0/8 and the
   host's own address, the NIC's interface everything else, both over one
   socket set (a TCP listener on 0.0.0.0 answers on both). smoltcp carries a
   second myos patch for that pair (`local-pair`: neither interface sends
   what is the other's, and a packet that waits for the other one does not
   silence its socket). `/net/ifaddrs` lists the interfaces, a line each in
   index order (`getifaddrs`, `if_nametoindex`):
   ```
   lo 127.0.0.1 255.0.0.0 up,loopback
   net0 10.0.2.15 255.255.255.0 up,broadcast
   ```
   (`-` for the address and mask until DHCP is done)

   netd also drives the virtual NICs of `/dev/tap` (`docs/wifi.md`: the
   Wi-Fi link), each one whose link is up, as `tap0` to `tap3` after
   `net0` in `/net/ifaddrs` (`user/netd/src/nics.rs`; it looks for them
   once a second). Each NIC has its own smoltcp interface and DHCP lease
   over the one socket set, and `lo` carries every NIC's address. The
   default route and `/net/ndb`'s address line are the preferred NIC's:
   `net0` while it has a lease, else the first other one with a lease; the
   `dns=` lines are the preferred NIC's servers, then the others'. A new
   connection takes the address of the NIC whose subnet holds its
   destination, else the preferred NIC's, and a packet leaves through the
   NIC its source address is: a third smoltcp patch (`multi-home`) makes
   an interface send only from its own addresses, leave the other NICs'
   to `lo`, and gives each DHCP socket the answers to its own NIC. The
   test is `multihome_smoke` (`user/c/test.sh`).
3. Apps: dial `/net/tcp|udp|icmp/{clone,ctl,data,status}` (and a tcp
   conversation's `peek`); a conversation's
   files are the user's whose process opened `clone` (an accepted
   connection: the listener's), no other user's (`docs/security.md`)

This feature adds a **libgloss userspace shim** that implements a trimmed BSD
sockets API on top of `/net`, so C ports (curl) link with `-lc -lgloss`.

### Shim ↔ `/net` mapping

| BSD call | `/net` action |
|----------|----------------|
| `socket(AF_INET, SOCK_STREAM, …)` | open `/net/tcp/clone`: the fd is the new conversation's `data` (its `/proc/self/fd` link names `N`); open `ctl`; return the **data fd** |
| `socket(…, SOCK_DGRAM, …)` | same with `/net/udp` |
| `connect(fd, sockaddr_in)` | write `connect a.b.c.d!port` to ctl; blocking waits for `connected`; **O_NONBLOCK** → `EINPROGRESS`, then `poll`/`select` **POLLOUT** (+ `SO_ERROR`) when netd reports Established. 127.0.0.0/8, the host's own address and 0.0.0.0 go over `lo`: a port nobody listens on is refused at once (**ECONNREFUSED**) |
| `recv(MSG_PEEK)` (TCP) | a read of the conversation's `peek`, which gives what a read of `data` would and leaves it there; it waits (or says `EAGAIN`) as an empty read of `data` does (UDP: issue #454) |
| `send`/`recv`/`read`/`write` | ordinary fd I/O on data; empty connected read blocks (in `SYS_POLL`) unless `O_NONBLOCK` (then EAGAIN); hangup → EOF. A TCP write takes what netd has room for (below): a blocking one waits for the rest, `O_NONBLOCK` gets a short write or EAGAIN, a hung-up peer EPIPE |
| UDP `bind`/`connect`/`sendto`/`recvfrom` | see UDP below |
| `close` | hangup via ctl (`hangup`) then close data (hook from `_close`) |
| `getaddrinfo` | `localhost` is `127.0.0.1` without a lookup; otherwise a DNS A lookup over `/net/udp` to the `dns=` servers of `/net/ndb` in turn, QEMU's `10.0.2.3` when it names none (same as `user/lib/dns.rs`), each asked up to four times, 5 s apart, for an answer (the wait is in `poll`; a query is one datagram, and CI's resolver has taken over 5 s for a name's first lookup); a server's first answer is final, so a name it does not know fails at once. The launcher moves QEMU's DNS to `10.0.2.4`, so the boot tests' lookups show the lease is followed. mlibc's own resolver (`docs/mlibc.md`) reads `/etc/hosts` (`localhost`) and `/etc/resolv.conf`, or without that file `/net/ndb`'s first `dns=`; `/etc/services` names the DNS port it looks up |
| `poll`/`select` | the kernel's `SYS_POLL`: netfs's `poll` hook reports **POLLIN** for bytes, a hangup or an accept not yet taken, **POLLOUT** once "connected" while the conversation has send room; the library adds **POLLOUT** for a connected UDP socket, arms a listener's `accept` before waiting and finishes a connect (`myos_socket_poll_prepare` / `_done`) |

A read of a UDP conversation's `data` returns one datagram (netfs keeps
their boundaries; bytes beyond the reader's buffer are dropped, as in
`recv`). The optional Linux layer maps Linux sockets onto the same files in
the kernel (`docs/linux-compat.md`, Sockets).

### UDP

Between netfs and netd every datagram carries a 12-byte header, Plan 9's
udp "headers" layout: remote address, local address, remote port, local
port. A conversation's `data` leaves it out: a read gives the bare
datagram, a write sends one to the connected peer, as plain Plan 9
`connect` users (the resolver, the Linux layer) expect. Its `hdata` has
the same datagrams with the header: a read tells the sender, a write names
the destination. The socket library's `recvfrom` and `sendto` go through
an fd of `hdata` it keeps to itself, close-on-exec; the socket's fd is the
`data` one. A program a socket is passed to by exec therefore reads and
writes bare datagrams with `read`/`write`, and its first socket call on the
fd (the library's table of sockets is per process) finds it by its name,
`/proc/self/fd/N` → `/net/udp/N/data` or `/net/tcp/N/data`: the library
takes the socket on from the conversation's state (a UDP socket's address
and peer asked of netd, a TCP one's status); what the exec'ing program kept
to itself, like `O_NONBLOCK`, is not passed on.

netd answers the library's UDP ctl commands, tagged `#<n>`, with a status
`#<n> ok <addr>!<port>[ <addr>!<port>]` (the local address after it, then
the peer's if connected) or `#<n> fail <why>`:

| ctl | |
|-----|--|
| `bind a.b.c.d!port[ reuse]` | port 0 picks a free one; the address may be any, 127.0.0.0/8, the host's or a broadcast one (`EADDRNOTAVAIL` otherwise); a taken port (on that address or any) is `EADDRINUSE` unless both binds asked for `SO_REUSEADDR`; a second bind `EINVAL` |
| `connect a.b.c.d!port` | the peer: the only source datagrams are taken from (netfs drops those it queued from others), and where a header-less send goes; binds a port first if none, and fixes the source address (127.0.0.1 for 127.0.0.0/8, else the host's) |
| `disconnect` | `connect(AF_UNSPEC)`: no peer, and the address and port bind did not choose given up (Linux's way) |
| `autobind` | the port a first send binds, asked first so `getsockname` knows it |
| `local` | nothing: the answer, for a socket taken on after an exec |

A datagram to a local port nobody bound is answered by an ICMP "port
unreachable", which netd sees on the loopback device: the connected socket
that sent it gets ECONNREFUSED. netfs keeps that error for the
conversation: it fails the next read or write, `poll` reports POLLERR, and
a read of `ctl` returns it once (`refused`), which is how the library takes
it (`SO_ERROR`, the failed call's errno). `shutdown` of a UDP socket is the
library's: reads return end of file, writes fail with EPIPE (no SIGPIPE,
which POSIX raises for streams only), an unconnected socket gets ENOTCONN
but the shutdown all the same, as on Linux.

Both directions of a TCP conversation are flow-controlled between netfs and
netd, so neither side ever drops bytes. Received data waits in the smoltcp
socket (64 KiB, the window offered to the peer) until netfs's 32 KiB buffer
has room (netfs hands the room back with `REQ_CREDIT` as readers drain it);
netd moves all it has room for per pass, in messages of up to 8 KiB. Sent
data: netfs lets a conversation's writers queue up to 32 KiB in netd; netd
moves it into the smoltcp socket (32 KiB) in order as the TCP window allows
and hands the room back with `REP_TXCREDIT`.
A write with no room left is refused, and the library waits for POLLOUT.

A conversation's index (`/net/tcp/N`) belongs to the process holding its
files until that process closes them: when the peer hangs up first, netd
closes the socket but keeps the index as a tombstone (status `hangup`,
reads end, writes fail) until the close, and only then may a new
connection, accepted or cloned, get it. Freeing it at the hangup let the
next accepted connection land on an index an HTTP server's keepalive
connection still held, which answered an SSH client's banner with an HTTP
response. An accepted connection whose peer is gone before netd saw it
established (its FIN came with the handshake's last ACK: a health check
that said its piece and left) is hung up the same way, so the process that
accepts it reads what came and the end at once instead of waiting for a
client until its own timeout with the index pinned. A connection its
process closed goes on closing in netd, apart from the conversation table:
its index is free in netd as in netfs, which freed its slot when the close
was acknowledged. netd sends what was written before the close and the FIN,
reads and drops what the peer still sends, and forgets the socket when the
TCP close completes, or aborts it 60 seconds after the close when the peer
never answers it (a scanner behind a firewall, a client that keeps a
half-closed connection), as Linux gives up on an orphaned connection.
Kept at its index as a closing conversation, it pinned the index (these
ran a VPS out of indices, with the status files looking empty), said
`hangup` about it after a new clone got it (netfs ignores what netd says
about a cloned index until netd answers the clone, issue #473), and was
lost when that clone overwrote it: its socket and buffers stayed in netd
for good, which ran out of memory after a few dozen loopback connections
(issue #483). A listener's
index is freed when its last holder closes it (a forked child's close
does not reach netd), its socket with it, and the connections it had
accepted that nobody took are reset; it used to live on with its socket,
so a restarted server shared its port with its own ghost, which answered
nothing.

An incoming connection is a conversation only once its handshake is done,
as accept on Linux only returns completed handshakes: netd answers the
SYN and keeps the half-open connection to itself (up to 64 of them, every
listener together; past that a SYN is answered with a reset, as Linux
drops a SYN when its queue is full), hands it to the listener's user once
the peer's ACK arrives, and aborts it after 15 seconds without one (Linux
gives up after about a minute). A SYN whose ACK never comes is the
internet's background noise, a scanner or a flood with forged sources on
any public port, and a connection reset during the handshake is a
scanner's connect-and-close; netd used to hand both out at the SYN, so
each pinned an index and woke the server (dropbear forks a process per
connection, and refuses connections past 30 unauthenticated ones) until
the server's own timeout: a scanned VPS held forty at a time. No boot
test leaves a handshake half-open (QEMU's user networking completes it
itself; issue #480).

netd puts a fresh listener on the port beside each connection it takes
off one, in whichever slot of smoltcp's socket set is free, and smoltcp
gives a segment to the first socket that accepts it. A SYN the peer sent
again (its SYN-ACK slow or lost; QEMU's user networking retransmits on the
host's clock while the guest stands still) went to that listener when its
slot came first: a second half-open connection for the same addresses,
whose SYN-ACK the peer reset, and the first connection with it (a listen
test's write failed with EPIPE). A fourth smoltcp patch
(`syn-to-connection`) hands a segment to a socket that is not listening
before the listeners, as Linux looks up a connection before a listener;
its test is smoltcp's own (`tests/ipv4.rs`).

`/net/convs` is netd's own table, a line per index in use (`N tcp|udp|icmp
[STATE] flags`: the TCP socket's state, `listen a.b.c.d!p` or `listen *!p`,
`peer a.b.c.d!p`, `accept` for a connection netd accepted, `held` while a
process has its files, `released` once it closed them, `connected`,
`hungup`, `closing Ns`; for a connection, where its bytes wait: `tx N`
sent ones netd has not yet put in the socket, `rxroom N` what netfs can
take, `local a.b.c.d!p` its local address, `sendq N` in the socket and not
yet acknowledged, `recvq N` received and not yet handed to netfs), then a
line per handshake in progress (`- tcp SynReceived peer a.b.c.d!p port P
handshake Ns`: not a conversation yet, no index) and per connection
closing after its process closed it (`- tcp STATE peer a.b.c.d!p closing
Ns tx N sendq N`, no index either), refreshed once a second. It shows
what the status files cannot: the connections still closing after their
processes let go, one accepted and not yet taken, the SYNs a scanner left
unanswered. When connections are reset at the accept, this is where the
indices went.

netd has 128 conversations (TCP, UDP and ICMP together; netfs has as many
slots), each a line of `/net/convs` while in use. A server on the
internet needs the room: a scanner's connection every few seconds, each
held for its server's request timeout (nginx 60 s, dropbear 60 s before
authentication) and then for the peer's half of the close.

TCP listens through netd's `announce` (`listen`/`accept`, dropbear's SSH
server, nginx): `bind` takes any address, one of 127.0.0.0/8 or the host's
own (`/net/ifaddrs`; another is `EADDRNOTAVAIL`), and `getsockname` reports
it; `listen` writes `announce a.b.c.d!port` (a bare `announce port` is any
address), and netd's listener takes only connections to that address.
`listen` waits for netd's answer: a port another listener has, on that
address or any, is `EADDRINUSE` there (Linux says so at `bind`; nginx and
the like take it from either), `SO_REUSEADDR` or not.
Most `SO_*`/`TCP_*` (`<netinet/tcp.h>`) are ignored. `AF_UNIX` stream sockets go over
`/net/unix` instead, served by the kernel without netd
(`docs/sockets-unix.md`).

### Smoke

`/bin/etc/socket_smoke` does `getaddrinfo` + `socket` + `connect` + HTTP GET to
`github.com:80` and prints `[ OK ] socket`. Wired into `/heap` (`CI_NEEDLES_STD`).

### curl

`ports/curl` builds trimmed static curl 8.11.1:

- Feature flags: `HTTP_ONLY`, many `CURL_DISABLE_*` (HTTP/2/3, unused protocols, auth, etc.)
- TLS: `USE_OPENSSL` against `ports/libressl` (LibreSSL, TLS 1.2 and 1.3) + Mozilla CA bundle (peer verify on)
- Sockets: libgloss shim (no kernel socket syscall)
- DNS: `getaddrinfo` in libgloss
- Clock: existing `gettimeofday` / RTC path (same as `/http`)
- Guest: `/bin/etc/curl`; its test fetches the launcher's test server over
  HTTPS, trusting the test CA alone (`--cacert /lib/myos-tests/test-ca.pem
  https://10.0.2.2:8767/hello.txt`, `docs/testing.md`)
- riscv64 links a small soft-float helper archive (`ports/curl/build-softfloat-riscv64.sh`)

### Build

`cargo build` runs these itself when `target/` lacks their outputs (curl and
`socket_smoke` are in every `core` image); by hand:

```sh
./toolchain/newlib/build.sh      # includes socket/inet/netdb/pollselect in libgloss
./scripts/build-c-hello.sh       # builds socket_smoke
./ports/libressl/build.sh
./ports/curl/build.sh
```

### Known gaps

- No IPv6; incomplete `getsockname` for TCP (returns INADDR_ANY)
- UDP: ICMP errors from the network (not the loopback device) reach no
  socket; no `MSG_PEEK`; a zero-length datagram reads as nothing waiting
  to a program that reads `data` without the library
- curl still a large ELF (~0.6–1.2MB stripped); many protocols disabled but not a tiny client
- Full QEMU smoke may not have been run on the builder box — rely on CI
