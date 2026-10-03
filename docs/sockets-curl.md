# Userspace BSD sockets + curl

## Design

myos has **no `socket()` syscall** and no kernel socket table. Networking is:

1. Kernel: virtio-net → `/dev/net0` + netfs Plan 9 `/net` + `/dev/netd` chrdev
2. Userspace `netd`: smoltcp over `/dev/net0`
3. Apps: dial `/net/tcp|udp|icmp/{clone,ctl,data,status}`

This feature adds a **libgloss userspace shim** that implements a trimmed BSD
sockets API on top of `/net`, so C ports (curl) link with `-lc -lgloss`.

### Shim ↔ `/net` mapping

| BSD call | `/net` action |
|----------|----------------|
| `socket(AF_INET, SOCK_STREAM, …)` | open `/net/tcp/clone`, read conv id, open `ctl` + `data`; return **data fd** |
| `socket(…, SOCK_DGRAM, …)` | same with `/net/udp` |
| `connect(fd, sockaddr_in)` | write `connect a.b.c.d!port` to ctl; blocking waits for `connected`; **O_NONBLOCK** → `EINPROGRESS`, then `poll`/`select` **POLLOUT** (+ `SO_ERROR`) when netd reports Established |
| `send`/`recv`/`read`/`write` | ordinary fd I/O on data; empty connected read blocks (in `SYS_POLL`) unless `O_NONBLOCK` (then EAGAIN); hangup → EOF. A TCP write takes what netd has room for (below): a blocking one waits for the rest, `O_NONBLOCK` gets a short write or EAGAIN, a hung-up peer EPIPE |
| `close` | hangup via ctl (`hangup`) then close data (hook from `_close`) |
| `getaddrinfo` | DNS A lookup over `/net/udp` to QEMU DNS `10.0.2.3:53` (same as `user/lib/dns.rs`) |
| `poll`/`select` | the kernel's `SYS_POLL`: netfs's `poll` hook reports **POLLIN** for bytes, a hangup or an accept not yet taken, **POLLOUT** once "connected" while the conversation has send room; the library adds **POLLOUT** for a connected UDP socket, arms a listener's `accept` before waiting and finishes a connect (`myos_socket_poll_prepare` / `_done`) |

A read of a UDP conversation's `data` returns one datagram (netfs keeps
their boundaries; bytes beyond the reader's buffer are dropped, as in
`recv`). The optional Linux layer maps Linux sockets onto the same files in
the kernel (`docs/linux-compat.md`, Sockets).

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

- No IPv6; incomplete `getsockname` for TCP/UDP (returns INADDR_ANY)
- curl still a large ELF (~0.6–1.2MB stripped); many protocols disabled but not a tiny client
- Full QEMU smoke may not have been run on the builder box — rely on CI
