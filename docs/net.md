# Networking

myos has TCP/IP over IPv4 and IPv6, but **no `socket()` syscall** and no
kernel socket table. A userspace daemon, netd, runs the TCP/IP stack
(smoltcp). The kernel's netfs module serves its connections as files under
`/net`, the Plan 9 way, and a socket library in libc maps the BSD sockets
API onto those files. This page covers netd, the `/net` files, the socket
library and curl. Local `AF_UNIX` sockets are in `docs/sockets-unix.md`.

## Layers

1. **Kernel.** The virtio-net driver gives `/dev/net0/data`. The netfs
   module (`modules/netfs`) serves `/net` and talks to netd over the
   `/dev/netd/data` channel.
2. **netd** (`user/netd`). It runs smoltcp over `/dev/net0/data` and takes
   its address, mask, gateway and DNS servers from DHCP.
3. **Programs.** They dial `/net/tcp|udp|icmp/{clone,ctl,data,status}`
   (and a TCP conversation's `peek`). A conversation's files belong to the
   user whose process opened `clone`; for an accepted connection, to the
   listener's user. No other user can open them (`docs/security.md`).

## Addresses and interfaces

`/net/ndb` shows the lease, Plan 9 style. It is empty until DHCP is done,
and again after the lease is lost. There is one `dns=` line per server;
the `ip6=` line and IPv6 servers appear once the NIC has an IPv6 address:

```
ip=10.0.2.15 ipmask=255.255.255.0 ipgw=10.0.2.2
ip6=fec0::5054:ff:fe12:3456/64 ip6gw=fe80::2
	dns=10.0.2.4
	dns=fec0::3
```

`/net/ifaddrs` lists the interfaces for `getifaddrs` and
`if_nametoindex`, in index order, each interface's IPv4 line first. The
address and mask read `-` until DHCP is done:

```
lo 127.0.0.1 255.0.0.0 up,loopback
net0 10.0.2.15 255.255.255.0 up,broadcast
lo ::1 ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff up,loopback
net0 fe80::5054:ff:fe12:3456 ffff:ffff:ffff:ffff:: up,broadcast
```

netd has a loopback interface, `lo`, beside the NIC. `lo` carries
127.0.0.0/8, ::1 and the host's own addresses; the NIC carries everything
else. Both share one socket set, so a TCP listener on 0.0.0.0 answers on
both.

### IPv6

- Each NIC has a link-local address from the MAC (modified EUI-64 under
  fe80::/64), and gets another by SLAAC in the prefix its router
  advertises (up to three router solicitations, 4 s apart).
- smoltcp ignores router advertisements, so the NIC's device
  (`user/net/src/lib.rs`) picks them out of the frames: the router, the
  prefix, the address lifetime and the DNS servers (RDNSS).
- It also picks out ICMP and ICMPv6 "destination unreachable", so a
  connect they answer fails at once, as on Linux. This matters on a host
  without IPv6, where QEMU answers every IPv6 SYN that way.
- QEMU's user network advertises fec0::/64 (host fec0::2). It advertises
  its DNS, fec0::3, only when the host has an IPv6 nameserver.
- Routing: link-local and off-prefix destinations go through the
  preferred NIC; a destination in a NIC's prefix goes through that NIC.

### Several NICs

netd also drives the virtual NICs of `/dev/tap` (the Wi-Fi link,
`docs/wifi.md`) whose link is up, as `tap0` to `tap3`
(`user/netd/src/nics.rs`). It looks for new NICs once a second.

- Each NIC has its own smoltcp interface and DHCP lease, over the one
  socket set. `lo` carries every NIC's address.
- The **preferred NIC** is `net0` while it has a lease, else the first
  other one with a lease. Its route is the default and its address is
  `/net/ndb`'s; its DNS servers are listed first.
- A new connection takes the address of the NIC whose subnet holds its
  destination, else the preferred NIC's. A packet leaves through the NIC
  that owns its source address.
- A board without `/dev/net0` (the Raspberry Pi 3 under QEMU) has `lo`
  alone. netd starts all the same, so a socket to another address fails
  at once instead of waiting on nobody (issue #538).

The test is `multihome_smoke` (`user/c/test.sh`).

### smoltcp patches

netd's smoltcp carries myos patches (`user/net/smoltcp/*.myos.patch`),
applied into `target/smoltcp-myos` by `prepare.sh` (`PORT_PREPARE`):

| Patch | What it does |
|-------|--------------|
| `arp-gateway` | a gateway outside the address's prefix (Hetzner Cloud gives a /32 and the gateway 172.31.1.1) |
| `local-pair` | `lo` and the NIC send only their own traffic, without blocking each other |
| `multi-home` | each interface sends only from its own addresses; each DHCP socket gets its own NIC's answers |
| `syn-to-connection` | a segment goes to an existing connection before the listeners, as on Linux (see "Listening") |
| `listen-family` | a listener or UDP socket on no address takes one IP version only (see "Listening") |

## The socket library

The socket calls are in `toolchain/libmyos/socket.c`, which libgloss and
mlibc share. C ports such as curl link with `-lc -lgloss`.

| BSD call | `/net` action |
|----------|----------------|
| `socket(AF_INET[6], SOCK_STREAM \| SOCK_DGRAM)` | open `/net/tcp/clone` or `/net/udp/clone`; the fd is the new conversation's `data` (its `/proc/self/fd` link names `N`) |
| `connect` | write `connect a.b.c.d!port` (or `fec0::2!port`) to `ctl`. `O_NONBLOCK` gives `EINPROGRESS`, then `POLLOUT` and `SO_ERROR`. Local addresses go over `lo`, so a port nobody listens on is `ECONNREFUSED` at once |
| `read`/`write`/`send`/`recv` | plain I/O on `data`. An empty read blocks unless `O_NONBLOCK`; a hangup is EOF; a write to a hung-up peer is `EPIPE`. A TCP write may be short (see "Flow control") |
| `recv(MSG_PEEK)` (TCP) | a read of the conversation's `peek` (UDP: issue #454) |
| `close` | writes `hangup` to `ctl`, then closes `data` |
| `poll`/`select` | the kernel's `SYS_POLL` over netfs's `poll` hook; the library arms a listener's accept and finishes a connect around it |
| `AF_INET6` | a socket on `::` takes IPv4 peers too, as `::ffff:a.b.c.d`, unless `IPV6_V6ONLY`. `inet_pton`/`inet_ntop`, `getnameinfo` (numeric) and `getifaddrs` take both families |

Most `SO_*`/`TCP_*` options are ignored: smoltcp's sockets keep their own
settings. The optional Linux layer maps Linux sockets onto the same `/net`
files in the kernel (`docs/linux-compat.md`, Sockets).

A program that gets a socket by exec reads and writes it as a plain fd.
The library finds it by the fd's name (`/proc/self/fd/N` →
`/net/udp/N/data`) and takes it on from the conversation's state; flags
such as `O_NONBLOCK` are not passed on.

### Name lookups

libgloss's `getaddrinfo` (`netdb.c`) resolves `localhost` to `127.0.0.1`
and `::1` and a literal address to itself, without a lookup. Anything
else is a DNS query over `/net/udp` to the `dns=` servers of `/net/ndb` in
turn, or QEMU's `10.0.2.3` when there are none. Each server is asked up
to four times, 5 s apart; its first answer to a query is final. Of two
queries sent together, one left unanswered a second after the other's
answer is sent again, up to the same four times: a lost AAAA datagram
left an `AF_UNSPEC` lookup with IPv4 alone once (CI #2369). `MYOS_DNS` in
the environment, `addr!port`, names the server to ask instead of the
lease's (a test's own on the loopback interface).

- `AF_INET` asks for A records, `AF_INET6` for AAAA.
- `AF_UNSPEC` asks for both, but only when the host has a global IPv6
  address, and lists the IPv4 addresses first. QEMU gives the guest IPv6
  whatever the host can reach, and many programs try only the first
  address.

The launcher moves QEMU's DNS to `10.0.2.4`, so the boot tests show that
the lease is followed. mlibc's own resolver (`docs/mlibc.md`) reads
`/etc/hosts`, `/etc/services` and `/etc/resolv.conf`, or else `/net/ndb`'s
first `dns=`.

## UDP

A read of a UDP conversation's `data` returns one datagram; bytes beyond
the reader's buffer are dropped. A write sends one datagram to the
connected peer.

`hdata` carries the same datagrams with a 36-byte header, Plan 9's udp
"headers" layout without the interface address: the remote address, the
local address (16 bytes each, IPv4 as `::ffff:a.b.c.d`), the remote port
and the local port. An all-zeros remote address means the connected peer.
The library's `recvfrom` and `sendto` go through an `hdata` fd it keeps to
itself.

netd answers the library's UDP `ctl` commands, tagged `#<n>`, with
`#<n> ok <local>[ <peer>]` or `#<n> fail <why>`:

| ctl | |
|-----|--|
| `bind a.b.c.d!port[ reuse][ v6only]` | the any address, a local one or an IPv4 broadcast one; port 0 picks a free port. A taken port is `EADDRINUSE` unless both binds asked for `SO_REUSEADDR` |
| `connect a.b.c.d!port` | the peer, the only source datagrams are taken from; binds a port first if none |
| `disconnect` | `connect(AF_UNSPEC)`: no peer, and what bind did not choose is given up |
| `autobind[ ::[ v6only]]` | binds the port a first send would, so `getsockname` knows it |
| `local` | no change: the answer tells a socket taken on after an exec its address |

A datagram to a local port nobody bound gets an ICMP "port unreachable".
The connected socket that sent it gets `ECONNREFUSED` on its next read or
write (`POLLERR`, and `refused` once on a read of `ctl`). `shutdown` is
done in the library: reads return EOF and writes fail with `EPIPE`,
without `SIGPIPE`.

## TCP

### Flow control

Both directions are flow-controlled between netfs and netd, so neither
drops bytes. Received data waits in the smoltcp socket (64 KiB, the
window offered to the peer) until netfs's 32 KiB buffer has room. A
conversation's writers may queue up to 32 KiB in netd; a write with no
room left is refused, and the library waits for `POLLOUT`.

The requests to netd (a clone, a `ctl` command, a send, a close) go
through a ring of 128 in netfs, which netd drains as it reads
`/dev/netd/data`. A burst of sends on a loaded guest fills it faster than
that; a clone, a command or a send that finds it full waits for netd's
next read, 5 s at most, and fails only past that, as with netd gone (a
full ring refused them outright, and `socket()` failed with `EIO`, issue
#574). A close never waits: it is queued for netd's next read instead.

### Conversation indices

netd has 128 conversations (TCP, UDP and ICMP together), as netfs has
slots. A conversation's index (`/net/tcp/N`) belongs to the process
holding its files until it closes them, so a new connection never lands
on an index a process still holds.

- When the peer hangs up first, netd keeps the index as a tombstone
  (status `hangup`) until the process closes it.
- When the process closes first, the index is free at once; netd finishes
  the close apart from the table and aborts it after 60 s without an
  answer, as Linux does for an orphaned connection.
- At most 16 sockets wait out TIME-WAIT (10 s); past that the oldest go,
  so a burst of loopback connections does not outgrow netd's 16 MiB heap.
- A listener is freed with its socket when its last holder closes it, and
  its unaccepted connections are reset, so a restarted server does not
  share its port with its old listener.

### Listening

`listen` writes `announce a.b.c.d!port` to `ctl` and waits for netd's
answer (`listening <port>`, which names the port for port 0):

- a bare `announce port` is any IPv4 address;
- `announce ::!port` is any IPv6 address and IPv4's too;
- `announce ::!port v6only` is IPv6's alone.

`bind` takes the any address, 127.0.0.0/8, ::1 or one of the host's own
(`/net/ifaddrs`); another is `EADDRNOTAVAIL`. A port another listener
would take the same connections on is `EADDRINUSE` at `listen` (Linux says
so at `bind`). The `listen-family` patch lets an IPv4 listener and an
IPv6-only one share a port (nginx's `listen 80` beside `listen [::]:80`).

A connection reaches the listener only once its handshake is done, as
Linux's accept returns only completed handshakes. Until then netd holds
it:

- up to 64 half-open connections in all, past which a SYN gets a reset;
  one without the final ACK is aborted after 15 s;
- two completed connections per listener wait for accept, and up to 32
  more queue in netd, accepted in the order they completed; past that
  they are reset;
- a connection whose peer already closed it is still accepted, with what
  it sent.

So scanners and SYN floods never reach the server or pin an index. No
boot test leaves a handshake half-open: QEMU's user networking completes
it itself (issue #480).

### `/net/convs`

`/net/convs` is netd's own table, refreshed once a second. It shows what
the status files cannot: connections still closing after their process
let go, ones not yet accepted, handshakes in progress. A line per index
in use reads `N tcp|udp|icmp [STATE] flags`, then lines starting with `-`
for handshakes (with their age in milliseconds) and orphaned closes. The
flags (`listen`, `peer`, `held`, `released`, `queue` with a listener's
offered connection, the one behind it and the offer's sequence, `tx`,
`sendq`, `recvq`, ...) are explained where netd writes them
(`user/netd/src/main.rs`).

## curl

`ports/curl` builds a trimmed static curl 8.11.1 (`/bin/etc/curl`):
`HTTP_ONLY` with many `CURL_DISABLE_*`, TLS 1.2 and 1.3 through
`ports/libressl` with the Mozilla CA bundle (peer verify on), the socket
library and libgloss's `getaddrinfo`. Its test fetches the launcher's test
server over HTTPS, trusting the test CA alone (`--cacert
/lib/myos-tests/test-ca.pem https://10.0.2.2:8767/hello.txt`,
`docs/testing.md`).

## Build and test

`cargo build` runs these scripts itself when `target/` lacks their
outputs. By hand:

```sh
./toolchain/newlib/build.sh      # socket/inet/netdb/pollselect in libgloss
./scripts/build-c-hello.sh       # builds socket_smoke
./ports/libressl/build.sh
./ports/curl/build.sh
```

`/bin/etc/socket_smoke` resolves `github.com`, connects and sends an HTTP
GET, and prints `[ OK ] socket`; `/heap` (`user/heap`) runs it. The IPv6,
multi-NIC and loopback tests are in `user/c/test.sh` (tag `netfs`).

## Limits

- `getsockname` for a connected TCP socket returns the any address.
- An `AF_INET6` TCP socket passed by exec is taken for an `AF_INET` one.
- UDP: ICMP errors from the network (not loopback) reach no socket; no
  `MSG_PEEK` (issue #454); a zero-length datagram reads as nothing to a
  program reading `data` without the library.
- IPv6: no duplicate address detection, DHCPv6, privacy addresses, second
  prefix per NIC, or use of the advertised MTU and hop limit. Fragmented
  packets are dropped. `sin6_scope_id` is not used for routing. The Linux
  layer has no `AF_INET6` listen or `IPV6_V6ONLY` (issue #611).
- curl is still a large ELF (~0.6–1.2 MB stripped).

## History

- **Closing connections freed their index** (issue #483). One kept at its
  index while closing pinned it, and was lost when a new clone took it
  (issue #473), leaking netd's memory.
- **Tombstones.** Freeing an index at the peer's hangup let a new
  connection land on an index a process still held (an SSH client got an
  HTTP server's answer).
- **Completed connections kept until accepted** (issue #630). Dropping
  them lost a slow server's requests.
- **Handshakes kept in netd** (issue #480). Handing a connection out at
  the SYN let scanners pin indices and dropbear's unauthenticated slots.
- **`syn-to-connection`.** A retransmitted SYN reached a fresh listener
  and reset the first connection; QEMU retransmits on the host's clock,
  which made this common.
