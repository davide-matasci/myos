# Networking

myos has TCP/IP over IPv4 and IPv6, but **no `socket()` syscall** and no
kernel socket table. A userspace daemon, netd, runs the TCP/IP stack
(smoltcp). The kernel's netfs module serves its connections as files under
`/net`, the Plan 9 way, and a socket library in libc maps the BSD sockets
API onto those files. This page covers netd, the `/net` files, the socket
library and curl. Local `AF_UNIX` sockets are in `docs/sockets-unix.md`.

## Layers

1. **Kernel.** The virtio-net driver gives `/dev/net0/data`. The netfs
   module serves the Plan 9 style `/net` and talks to netd over the
   `/dev/netd/data` channel.
2. **netd** (`user/netd`). It runs smoltcp over `/dev/net0/data`. It takes
   its address, mask and default gateway from DHCP, and hands the lease to
   netfs (`/net/ndb`, below).
3. **Programs.** They dial `/net/tcp|udp|icmp/{clone,ctl,data,status}`
   (and a TCP conversation's `peek`). A conversation's files belong to the
   user whose process opened `clone`; for an accepted connection, to the
   listener's user. No other user can open them (`docs/security.md`).

On top of this, a **userspace socket library** implements a trimmed BSD
sockets API over `/net`, so C ports such as curl link with `-lc -lgloss`
(table under "The socket library").

## Addresses and interfaces

`/net/ndb` shows the lease, Plan 9 style:

```
ip=10.0.2.15 ipmask=255.255.255.0 ipgw=10.0.2.2
	dns=10.0.2.4
```

There is one `dns=` line per DNS server the lease names. The file is empty
until DHCP is done, and again after the lease is lost.

netd has a second smoltcp interface, `lo`, on a loopback device. `lo`
carries 127.0.0.0/8 and the host's own addresses. The NIC's interface
carries everything else. Both share one socket set, so a TCP listener on
0.0.0.0 answers on both.

`/net/ifaddrs` lists the interfaces, one line each, in index order. It
serves `getifaddrs` and `if_nametoindex`:

```
lo 127.0.0.1 255.0.0.0 up,loopback
net0 10.0.2.15 255.255.255.0 up,broadcast
```

The address and mask read `-` until DHCP is done. A line per IPv6 address
follows (see "IPv6"). Each interface's lines stay together, its IPv4 line
first:

```
lo ::1 ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff up,loopback
net0 fe80::5054:ff:fe12:3456 ffff:ffff:ffff:ffff:: up,broadcast
net0 fec0::5054:ff:fe12:3456 ffff:ffff:ffff:ffff:: up,broadcast
```

## IPv6

smoltcp carries IPv6 beside IPv4 (`proto-ipv6`, neighbor discovery).

- Each NIC has a link-local address from the start. It is derived from the
  MAC: the modified EUI-64 interface identifier under fe80::/64.
- Each NIC also gets an address of its own by SLAAC (RFC 4862), in the
  /64 prefix that its link's router advertises. netd sends up to three
  router solicitations, 4 s apart, until one answers.
- smoltcp leaves router advertisements alone, so the NIC's device
  (`user/net/src/lib.rs`, `Net0Device`) picks them out of the frames. An
  advertisement names the default router (its link-local address), the
  prefix, how long its addresses are valid (the address goes when that
  runs out) and the DNS servers (RDNSS, RFC 8106).
- QEMU's user network advertises fec0::/64 and its host fec0::2. It
  advertises its DNS, fec0::3, only when the host has an IPv6 nameserver
  to forward to (libslirp's condition).

`/net/ndb` shows the preferred NIC's IPv6 address on an `ip6=` line under
the IPv4 one, and its IPv6 servers as `dns=` lines after the IPv4 ones.
The preferred NIC is the wired one with a router, else the first other
one.

```
ip=10.0.2.15 ipmask=255.255.255.0 ipgw=10.0.2.2
ip6=fec0::5054:ff:fe12:3456/64 ip6gw=fe80::2
	dns=10.0.2.4
	dns=fec0::3
```

`lo` carries ::1 and every NIC's IPv6 addresses too. Routing:

- a connection to a link-local address leaves through the preferred NIC
  (scope ids are not looked at);
- one to an address in a NIC's prefix leaves through that NIC;
- any other goes through the preferred NIC's router.

A connect that an ICMP or ICMPv6 "destination unreachable" answers fails
at once, as on Linux. smoltcp hands ICMP errors to no socket and would
send the SYN again until the caller gave up, so the NIC's device picks
those messages out of the frames too. This matters in CI: on a host
without IPv6, QEMU's user network answers every SYN to an IPv6 address
this way, and c-ares (Alpine's curl) lists the IPv6 addresses first.

What IPv6 lacks is listed under "Limits".

## Several NICs

netd also drives the virtual NICs of `/dev/tap` (the Wi-Fi link,
`docs/wifi.md`), each one whose link is up. They show as `tap0` to `tap3`
after `net0` in `/net/ifaddrs` (`user/netd/src/nics.rs`). netd looks for
them once a second.

A board without `/dev/net0` (the Raspberry Pi 3 under QEMU) has `lo`
alone. netd starts all the same, so loopback works, and a socket to
another address fails at once (a TCP connect is refused, a lookup finds
no server) instead of waiting on a netd that never came (issue #538). A
wired NIC that appears later is taken at the next look.

With several NICs:

- Each NIC has its own smoltcp interface and DHCP lease, over the one
  socket set. `lo` carries every NIC's address.
- The default route and `/net/ndb`'s address line are the preferred NIC's:
  `net0` while it has a lease, else the first other one with a lease.
- The `dns=` lines list the preferred NIC's servers, then the others'.
- A new connection takes the address of the NIC whose subnet holds its
  destination, else the preferred NIC's.
- A packet leaves through the NIC that owns its source address.

The test is `multihome_smoke` (`user/c/test.sh`).

## smoltcp patches

netd's smoltcp carries five myos patches (`user/net/smoltcp/*.myos.patch`).
`prepare.sh` applies them into `target/smoltcp-myos` before netd is built (`PORT_PREPARE`).

| Patch | What it does |
|-------|--------------|
| `arp-gateway` | accepts ARP from the router of a route, so the gateway may lie outside the address's prefix (Hetzner Cloud gives a /32 and the gateway 172.31.1.1) |
| `local-pair` | for `lo` and the NIC: neither interface sends what is the other's, and a packet that waits for the other one does not silence its socket |
| `multi-home` | an interface sends only from its own addresses and leaves the other NICs' to `lo`; each DHCP socket gets the answers to its own NIC |
| `syn-to-connection` | hands a segment to a socket that is not listening before the listeners, as Linux looks up a connection before a listener (see "Listening"); its test is smoltcp's own (`tests/ipv4.rs`) |
| `listen-family` | a TCP listener or a UDP socket on no address takes one IP version only (see "Listening") |

## The socket library

The socket calls are in `toolchain/libmyos/socket.c`, which libgloss and
mlibc share; libgloss's `getaddrinfo` is in its `netdb.c` (mlibc has its
own resolver, below).

| BSD call | `/net` action |
|----------|----------------|
| `socket(AF_INET, SOCK_STREAM, …)` | open `/net/tcp/clone`: the fd is the new conversation's `data` (its `/proc/self/fd` link names `N`); open `ctl`; return the **data fd**. `AF_INET6` is the same conversation |
| `socket(…, SOCK_DGRAM, …)` | same with `/net/udp` |
| `connect(fd, sockaddr_in)` | write `connect a.b.c.d!port` (`sockaddr_in6`: `connect fec0::2!port`) to ctl. Blocking: wait for `connected`. **O_NONBLOCK**: `EINPROGRESS`, then `poll`/`select` **POLLOUT** (+ `SO_ERROR`) when netd reports Established. 127.0.0.0/8, ::1, the host's own addresses, 0.0.0.0 and :: go over `lo`: a port nobody listens on is refused at once (**ECONNREFUSED**) |
| `recv(MSG_PEEK)` (TCP) | a read of the conversation's `peek`, which gives what a read of `data` would and leaves it there; it waits (or says `EAGAIN`) as an empty read of `data` does (UDP: issue #454) |
| `send`/`recv`/`read`/`write` | ordinary fd I/O on data. An empty connected read blocks (in `SYS_POLL`) unless `O_NONBLOCK` (then EAGAIN); hangup → EOF. A TCP write takes what netd has room for (see "Flow control"): a blocking one waits for the rest, `O_NONBLOCK` gets a short write or EAGAIN, a hung-up peer EPIPE |
| UDP `bind`/`connect`/`sendto`/`recvfrom` | see "UDP" |
| `close` | hangup via ctl (`hangup`), then close data (hook from `_close`) |
| `getaddrinfo` | see "Name lookups" |
| `AF_INET6` | an IPv6 socket on `::` takes IPv4 peers too, which it names `::ffff:a.b.c.d`, as on Linux, unless `IPV6_V6ONLY` (`setsockopt`, taken at the `bind`). One connected or sending to an IPv4-mapped address talks IPv4 to netd. `inet_pton`/`inet_ntop` (RFC 5952 text), `getnameinfo` (numeric) and `getifaddrs` take both families; a link-local address's `sin6_scope_id` is its interface's index |
| `poll`/`select` | the kernel's `SYS_POLL`. netfs's `poll` hook reports **POLLIN** for bytes, a hangup or an accept not yet taken, and **POLLOUT** once "connected" while the conversation has send room. The library adds **POLLOUT** for a connected UDP socket, arms a listener's `accept` before waiting and finishes a connect (`myos_socket_poll_prepare` / `_done`) |

Most `SO_*`/`TCP_*` options (`<netinet/tcp.h>`) are ignored. `AF_UNIX`
stream sockets go over `/net/unix` instead, served by the kernel without
netd (`docs/sockets-unix.md`). The optional Linux layer maps Linux sockets
onto the same `/net` files in the kernel (`docs/linux-compat.md`,
Sockets).

### Name lookups

`getaddrinfo` resolves:

- `localhost` to `127.0.0.1` and `::1`, without a lookup;
- a literal address to itself, in its own family;
- anything else by a DNS lookup over `/net/udp`.

The lookup asks the `dns=` servers of `/net/ndb` in turn (IPv4 or IPv6
ones). When `/net/ndb` names none, it asks QEMU's `10.0.2.3`; its A
lookups match `user/lib/dns.rs`'s. Each server is asked up to four times,
5 s apart, for an answer. The wait is in `poll`, and a query is one
datagram; CI's resolver has taken over 5 s for a name's first lookup. A
server's first answer is final, so a name it does not know fails at once.

- `AF_INET` asks for A records, `AF_INET6` for AAAA.
- `AF_UNSPEC` asks for both at once (two queries on one socket, as glibc
  does; the second may take a second longer than the first), but only when
  the host has an IPv6 address beyond ::1 and the link-local ones. It
  lists the IPv4 addresses first. QEMU's user network gives the guest an
  IPv6 address whatever the host can reach, and a program that tries only
  the first address would otherwise try IPv6 first.

The launcher moves QEMU's DNS to `10.0.2.4`, so the boot tests' lookups
show that the lease is followed.

mlibc's own resolver (`docs/mlibc.md`) reads `/etc/hosts` (`localhost`)
and `/etc/resolv.conf`, or, without that file, `/net/ndb`'s first `dns=`.
`/etc/services` names the DNS port it looks up. It asks AAAA records for
`AF_INET6` and A records otherwise.

## UDP

A read of a UDP conversation's `data` returns one datagram: netfs keeps
the datagram boundaries. Bytes beyond the reader's buffer are dropped, as
in `recv`.

### Datagram header

Between netfs and netd every datagram carries a 36-byte header. It is
Plan 9's udp "headers" layout without its interface address:

1. remote address (16 bytes; an IPv4 one as `::ffff:a.b.c.d`);
2. local address (16 bytes, same form);
3. remote port;
4. local port.

An all-zeros remote address means none: the connected peer without a port
(a `data` write), or `::` with one (this host, as `::ffff:0.0.0.0` is).

A conversation has two data files:

- `data` leaves the header out. A read gives the bare datagram, and a
  write sends one to the connected peer, as plain Plan 9 `connect` users
  (the resolver, the Linux layer) expect.
- `hdata` has the same datagrams with the header. A read tells the
  sender, and a write names the destination.

The socket library's `recvfrom` and `sendto` go through an fd of `hdata`
that it keeps to itself, close-on-exec. The socket's own fd is the `data`
one.

### Sockets passed by exec

A program that gets a socket by exec reads and writes bare datagrams with
`read`/`write`. The library's table of sockets is per process, so its
first socket call on the fd finds the socket by the fd's name:
`/proc/self/fd/N` → `/net/udp/N/data` or `/net/tcp/N/data`. The library
then takes the socket on from the conversation's state: a UDP socket's
address and peer, asked of netd, or a TCP one's status. What the exec'ing
program kept to itself, like `O_NONBLOCK`, is not passed on.

### ctl commands

netd answers the library's UDP ctl commands, tagged `#<n>`, with a status:
`#<n> ok <addr>!<port>[ <addr>!<port>]` (the local address, then the
peer's if connected) or `#<n> fail <why>`.

| ctl | |
|-----|--|
| `bind a.b.c.d!port[ reuse][ v6only]` | port 0 picks a free one. The address may be any (`0.0.0.0`: IPv4's; `::`: IPv6's, and IPv4's too unless `v6only`), 127.0.0.0/8, ::1, the host's or an IPv4 broadcast one (`EADDRNOTAVAIL` otherwise). A taken port (one another conversation would take a datagram to: on that address or any, of a version both take) is `EADDRINUSE` unless both binds asked for `SO_REUSEADDR`. A second bind is `EINVAL` |
| `connect a.b.c.d!port` | the peer: the only source datagrams are taken from (netfs drops those it queued from others), and where a header-less send goes. Binds a port first if none, and fixes the source address (127.0.0.1 for 127.0.0.0/8, ::1 for ::1, else the host's of the peer's version) |
| `disconnect` | `connect(AF_UNSPEC)`: no peer, and the address and port that bind did not choose are given up (Linux's way) |
| `autobind[ ::[ v6only]]` | the port a first send binds, asked first so `getsockname` knows it; an IPv6 socket's is on `::` (a first send to an IPv6 address binds there too) |
| `local` | nothing: the answer, for a socket taken on after an exec |

### Errors and shutdown

A datagram to a local port nobody bound is answered by an ICMP "port
unreachable", which netd sees on the loopback device. The connected socket
that sent it gets ECONNREFUSED. netfs keeps that error for the
conversation:

- it fails the next read or write;
- `poll` reports POLLERR;
- a read of `ctl` returns it once (`refused`), which is how the library
  takes it (`SO_ERROR`, the failed call's errno).

`shutdown` of a UDP socket is done in the library. Reads return end of
file. Writes fail with EPIPE, without SIGPIPE, which POSIX raises for
streams only. An unconnected socket gets ENOTCONN but is shut down all the
same, as on Linux.

## TCP

### Flow control

Both directions of a TCP conversation are flow-controlled between netfs
and netd, so neither side ever drops bytes.

- **Received data** waits in the smoltcp socket (64 KiB, the window
  offered to the peer) until netfs's 32 KiB buffer has room. netfs hands
  the room back with `REQ_CREDIT` as readers drain it. netd moves all it
  has room for per pass, in messages of up to 8 KiB.
- **Sent data**: netfs lets a conversation's writers queue up to 32 KiB in
  netd. netd moves it into the smoltcp socket (32 KiB) in order, as the
  TCP window allows, and hands the room back with `REP_TXCREDIT`. A write
  with no room left is refused, and the library waits for POLLOUT.

### Conversation indices

netd has 128 conversations (TCP, UDP and ICMP together; netfs has as many
slots), each a line of `/net/convs` while in use. A server on the
internet needs the room: a scanner connects every few seconds, and each
connection is held for its server's request timeout (nginx 60 s, dropbear
60 s before authentication) and then for the peer's half of the close.

A conversation's index (`/net/tcp/N`) belongs to the process holding its
files until that process closes them. Only then may a new connection,
accepted or cloned, get it.

- **The peer hangs up first.** netd closes the socket but keeps the index
  as a tombstone until the close: status `hangup`, reads end, writes
  fail.
- **The peer is gone before netd saw the connection established** (its FIN
  came with the handshake's last ACK, as from a health check that said its
  piece and left). netd hangs it up the same way, so the process that
  accepts it reads what came and the end at once, instead of waiting for
  the client until its own timeout with the index pinned.
- **The process closes first.** The connection goes on closing in netd,
  apart from the conversation table. Its index is free in netd as in
  netfs, which freed its slot when the close was acknowledged. netd sends
  what was written before the close and the FIN, reads and drops what the
  peer still sends, and forgets the socket when the TCP close completes.
  It aborts the socket 60 seconds after the close when the peer never
  answers (a scanner behind a firewall, a client that keeps a half-closed
  connection), as Linux gives up on an orphaned connection.
- **TIME-WAIT.** The side that closed first waits out smoltcp's 10 seconds
  of TIME-WAIT with its socket and buffers (96 KiB). At most 16 wait;
  past that the oldest go at once, as Linux skips TIME-WAIT past
  `tcp_max_tw_buckets`. Over loopback netd is both ends, so every
  connection leaves one, and a burst of short connections would otherwise
  outgrow netd's 16 MiB heap.
- **A listener** is freed when its last holder closes it (a forked child's
  close does not reach netd), and its socket with it. The connections it
  had accepted that nobody took are reset. This way a restarted server
  does not share its port with its old listener.

### Listening

TCP listens through netd's `announce` (`listen`/`accept`: dropbear's SSH
server, nginx).

- `bind` takes the any address, one of 127.0.0.0/8, ::1 or the host's own
  (`/net/ifaddrs`); another is `EADDRNOTAVAIL`. `getsockname` reports it.
- `listen` writes `announce a.b.c.d!port`:
  - a bare `announce port` is any IPv4 address;
  - `announce ::!port` is any IPv6 address and IPv4's too;
  - `announce ::!port v6only` is IPv6's alone;
  - port 0 (a `bind` to it, or none) is a free one; netd's answer names
    it (`listening <port>`), and `getsockname` reports it.
- netd's listener takes only connections to that address, of that version.
  The `listen-family` smoltcp patch makes this hold for a listener on no
  address too, so an IPv4 server and an IPv6-only one share a port
  (nginx's `listen 80` beside `listen [::]:80`, whose `ipv6only` is on).
- `listen` waits for netd's answer. A port another listener would take a
  connection to (on that address or any, of a version both take) is
  `EADDRINUSE` there, `SO_REUSEADDR` or not. Linux says so at `bind`;
  nginx and the like take it from either.

An incoming connection becomes a conversation only once its handshake is
done, as accept on Linux returns only completed handshakes:

- netd answers the SYN and keeps the half-open connection to itself. It
  keeps up to 64 of them, every listener together; past that a SYN is
  answered with a reset, as Linux drops a SYN when its queue is full.
- It hands the connection to the listener's user once the peer's ACK
  arrives, and aborts it after 15 seconds without one (Linux gives up
  after about a minute).
- Two of a listener's connections wait for its user's accept at a time. A
  further completed one waits with the half-open ones, 32 per listener at
  most. Past that it is reset, so that a server that stopped accepting
  leaves the other ports their handshakes.
- A connection waiting for its accept stays even when its peer has closed
  it: accept returns it, and its user reads what came and the end, as on
  Linux.

So a SYN whose ACK never comes (a scanner, or a flood with forged sources,
on any public port) and a connection reset during the handshake (a
scanner's connect-and-close) never reach the server. No boot test leaves
a handshake half-open: QEMU's user networking completes it itself (issue
#480).

netd puts a fresh listener on the port beside each connection it takes off
one, in whichever slot of smoltcp's socket set is free, and smoltcp gives
a segment to the first socket that accepts it. The `syn-to-connection`
patch makes smoltcp look up the connection before the listeners, so a SYN
the peer sent again (its SYN-ACK slow or lost) reaches the existing
connection rather than the new listener.

### `/net/convs`

`/net/convs` is netd's own table, refreshed once a second. It shows what
the status files cannot: the connections still closing after their
processes let go, one accepted and not yet taken, the SYNs a scanner left
unanswered. When connections are reset at the accept, this is where the
indices went.

It has a line per index in use:

```
N tcp|udp|icmp [STATE] flags
```

`STATE` is the TCP socket's state. The flags are:

| Flag | Meaning |
|------|---------|
| `listen a.b.c.d!p` | listening on that address |
| `listen *!p` | listening on any IPv4 address |
| `listen [::]!p` | listening on any IPv6 address and IPv4 |
| `listen [::]6!p` | listening on IPv6 only |
| `peer a.b.c.d!p` | the peer |
| `accept` | a connection netd accepted |
| `held` | a process has its files |
| `released` | the process closed them |
| `connected`, `hungup`, `closing Ns` | the connection's state |
| `tx N` | sent bytes netd has not yet put in the socket |
| `rxroom N` | what netfs can take |
| `local a.b.c.d!p` | the local address |
| `sendq N` | bytes in the socket and not yet acknowledged |
| `recvq N` | received bytes not yet handed to netfs |

Then come lines without an index:

- one per handshake in progress, `- tcp SynReceived peer a.b.c.d!p port P
  handshake Ns` (not a conversation yet), or `Established` or `CloseWait`
  for a completed one waiting for its listener's accept;
- one per connection still closing after its process closed it, `- tcp
  STATE peer a.b.c.d!p closing Ns tx N sendq N`.

## curl

`ports/curl` builds a trimmed static curl 8.11.1:

- Feature flags: `HTTP_ONLY`, many `CURL_DISABLE_*` (HTTP/2/3, unused
  protocols, auth, etc.).
- TLS: `USE_OPENSSL` against `ports/libressl` (LibreSSL, TLS 1.2 and 1.3)
  with the Mozilla CA bundle (peer verify on).
- Sockets: the userspace socket library (no kernel socket syscall).
- DNS: `getaddrinfo` in libgloss.
- Clock: the existing `gettimeofday` / RTC path (the same as `/http`).
- In the guest: `/bin/etc/curl`. Its test fetches the launcher's test
  server over HTTPS, trusting the test CA alone (`--cacert
  /lib/myos-tests/test-ca.pem https://10.0.2.2:8767/hello.txt`,
  `docs/testing.md`).

## Build

`cargo build` runs these scripts itself when `target/` lacks their outputs
(curl and `socket_smoke` are in every `core` image). By hand:

```sh
./toolchain/newlib/build.sh      # includes socket/inet/netdb/pollselect in libgloss
./scripts/build-c-hello.sh       # builds socket_smoke
./ports/libressl/build.sh
./ports/curl/build.sh
```

## Smoke test

`/bin/etc/socket_smoke` does `getaddrinfo`, `socket`, `connect` and an
HTTP GET to `github.com:80`, and prints `[ OK ] socket`. `/heap`
(`user/heap`) runs it.

## Limits

- `getsockname` for TCP is incomplete: it returns the bound address, and
  the any address for a connected socket.
- An `AF_INET6` TCP socket passed by exec is taken for an `AF_INET` one (a
  UDP one's family is its address's).
- UDP: ICMP errors from the network (not the loopback device) reach no
  socket; no `MSG_PEEK` (issue #454); a zero-length datagram reads as
  nothing waiting to a program that reads `data` without the library.
- IPv6: no duplicate address detection, DHCPv6, temporary (privacy)
  addresses, second prefix per NIC, or use of the advertisement's MTU and
  hop limit. A packet that comes in fragments is dropped. A link-local
  `sin6_scope_id` is not used. The Linux layer has no `AF_INET6` listen or
  `IPV6_V6ONLY` (issue #611).
- curl is still a large ELF (~0.6–1.2MB stripped): many protocols are
  disabled, but it is not a tiny client.

## History

- **TIME-WAIT cap.** A burst of short loopback connections (the IPv6
  smoke's, run back to back) piled up more TIME-WAIT sockets than netd's
  16 MiB heap holds, and netd died of the failed allocation, every socket
  call after it waiting on nobody.
- **Closing connections kept their index** (issue #483). A connection its
  process had closed used to stay at its index while closing. It pinned
  the index (this ran a VPS out of indices, with the status files looking
  empty), and said `hangup` about it after a new clone got it (netfs
  ignores what netd says about a cloned index until netd answers the
  clone, issue #473). It was lost when that clone overwrote it, and its
  socket and buffers stayed in netd for good, which ran out of memory
  after a few dozen loopback connections.
- **Tombstones.** Freeing an index at the peer's hangup let the next
  accepted connection land on an index that an HTTP server's keepalive
  connection still held, which answered an SSH client's banner with an
  HTTP response.
- **Listeners freed with their socket.** A listener used to live on with
  its socket after its server closed it, so a restarted server shared its
  port with its own ghost, which answered nothing.
- **Completed connections kept until accepted** (issue #630). netd used to
  drop a completed connection whose peer had closed it before the accept,
  and reset those past two waiting, so a server that fell behind lost
  them, requests and all: the listen test's last connection.
- **Handshakes kept in netd** (issue #480). netd used to hand a connection
  out at the SYN, so every scanner SYN and every connect-and-close pinned
  an index and woke the server until the server's own timeout (dropbear
  forks a process per connection and refuses connections past 30
  unauthenticated ones). A scanned VPS held forty at a time.
- **`syn-to-connection`.** A SYN the peer sent again went to the fresh
  listener when its slot came first: a second half-open connection for
  the same addresses, whose SYN-ACK the peer reset, and the first
  connection with it (a listen test's write failed with EPIPE). QEMU's
  user networking retransmits on the host's clock while the guest stands
  still, which made this common.
