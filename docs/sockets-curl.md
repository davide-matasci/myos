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
3. Apps: dial `/net/tcp|udp|icmp/{clone,ctl,data,status}`; a conversation's
   files are the user's whose process read `clone` (an accepted
   connection: the listener's), no other user's (`docs/security.md`)

This feature adds a **libgloss userspace shim** that implements a trimmed BSD
sockets API on top of `/net`, so C ports (curl) link with `-lc -lgloss`.

### Shim ↔ `/net` mapping

| BSD call | `/net` action |
|----------|----------------|
| `socket(AF_INET, SOCK_STREAM, …)` | open `/net/tcp/clone`, read conv id, open `ctl` + `data`; return **data fd** |
| `socket(…, SOCK_DGRAM, …)` | same with `/net/udp` |
| `connect(fd, sockaddr_in)` | write `connect a.b.c.d!port` to ctl; blocking waits for `connected`; **O_NONBLOCK** → `EINPROGRESS`, then `poll`/`select` **POLLOUT** (+ `SO_ERROR`) when netd reports Established. 127.0.0.0/8, the host's own address and 0.0.0.0 go over `lo`: a port nobody listens on is refused at once (**ECONNREFUSED**) |
| `send`/`recv`/`read`/`write` | ordinary fd I/O on data; empty connected read blocks (in `SYS_POLL`) unless `O_NONBLOCK` (then EAGAIN); hangup → EOF. A TCP write takes what netd has room for (below): a blocking one waits for the rest, `O_NONBLOCK` gets a short write or EAGAIN, a hung-up peer EPIPE |
| UDP `bind`/`connect`/`sendto`/`recvfrom` | see UDP below |
| `close` | hangup via ctl (`hangup`) then close data (hook from `_close`) |
| `getaddrinfo` | `localhost` is `127.0.0.1` without a lookup; otherwise a DNS A lookup over `/net/udp` to the `dns=` servers of `/net/ndb` in turn, QEMU's `10.0.2.3` when it names none (same as `user/lib/dns.rs`), each given 5 s to answer (the wait is in `poll`); a server's first answer is final, so a name it does not know fails at once. The launcher moves QEMU's DNS to `10.0.2.4`, so the boot tests' lookups show the lease is followed |
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
socket until netfs's 8 KiB buffer has room (netfs hands the room back with
`REQ_CREDIT` as readers drain it). Sent data: netfs lets a conversation's
writers queue up to 8 KiB in netd; netd moves it into the smoltcp socket in
order as the TCP window allows and hands the room back with `REP_TXCREDIT`.
A write with no room left is refused, and the library waits for POLLOUT.

TCP listens through netd's `announce` (`listen`/`accept`, dropbear's SSH
server). Most `SO_*`/`TCP_*` are ignored. `AF_UNIX` stream sockets go over
`/net/unix` instead, served by the kernel without netd
(`docs/sockets-unix.md`).

### Smoke

`/bin/etc/socket_smoke` does `getaddrinfo` + `socket` + `connect` + HTTP GET to
`example.com:80` and prints `[ OK ] socket`. Wired into `/heap` (`CI_NEEDLES_STD`).

### curl

`ports/curl` builds trimmed static curl 8.11.1:

- Feature flags: `HTTP_ONLY`, many `CURL_DISABLE_*` (HTTP/2/3, unused protocols, auth, etc.)
- TLS: `USE_MBEDTLS` against `ports/mbedtls` + Mozilla CA bundle (peer verify on)
- Sockets: libgloss shim (no kernel socket syscall)
- DNS: `getaddrinfo` in libgloss
- Clock: existing `gettimeofday` / RTC path (same as `/http`)
- Guest: `/bin/etc/curl`; CI types
  `curl -fsS --connect-timeout 30 --max-time 90 -o /tmp/curl-ex.html https://example.com/; cat /tmp/curl-ex.html`
- riscv64 links a small soft-float helper archive (`ports/curl/build-softfloat-riscv64.sh`)

### Build

`cargo build` runs these itself when `target/` lacks their outputs (curl and
`socket_smoke` are in every `core` image); by hand:

```sh
./toolchain/newlib/build.sh      # includes socket/inet/netdb/pollselect in libgloss
./scripts/build-c-hello.sh       # builds socket_smoke
./ports/mbedtls/build.sh
./ports/curl/build.sh
```

### Known gaps

- No IPv6; incomplete `getsockname` for TCP (returns INADDR_ANY)
- UDP: ICMP errors from the network (not the loopback device) reach no
  socket; no `MSG_PEEK`; a zero-length datagram reads as nothing waiting
  to a program that reads `data` without the library
- curl still a large ELF (~0.6–1.2MB stripped); many protocols disabled but not a tiny client
- Full QEMU smoke may not have been run on the builder box — rely on CI
