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
| `connect(fd, sockaddr_in)` | write `connect a.b.c.d!port` to ctl; poll `status` for `connected` |
| `send`/`recv`/`read`/`write` | ordinary fd I/O on data; empty connected read blocks unless `O_NONBLOCK` (then EAGAIN); hangup → EOF |
| `close` | hangup via ctl (`hangup`) then close data (hook from `_close`) |
| `getaddrinfo` | DNS A lookup over `/net/udp` to QEMU DNS `10.0.2.3:53` (same as `user/lib/dns.rs`) |
| `poll`/`select` | userspace busy-wait; reports readiness (matches existing http busy-poll) |

Outbound TCP/UDP first. `listen`/`accept` return `EOPNOTSUPP`. Most `SO_*`/`TCP_*` are ignored.

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

```sh
./toolchain/newlib/build.sh      # includes socket/inet/netdb/pollselect in libgloss
./scripts/build-c-hello.sh       # builds socket_smoke
./ports/mbedtls/build.sh
./ports/curl/build.sh
```

### Known gaps

- No inbound listen/accept; no IPv6; incomplete `getsockname` (returns INADDR_ANY)
- poll/select: sockets use netfs RX size / hangup; other fds still always-ready
- curl still a large ELF (~0.6–1.2MB stripped); many protocols disabled but not a tiny client
- Full QEMU smoke may not have been run on the builder box — rely on CI
