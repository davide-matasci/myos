# Lynx for myos

Upstream **Lynx 2.9.3** (fetched at build; not vendored). Cross-built with
newlib + myos libgloss **userspace BSD sockets** (`/net`) + static ncurses +
**LibreSSL** for HTTPS.

## Layout

| Script | Role |
|--------|------|
| `fetch.sh` | Download pinned tarball → `target/lynx-src` (SHA-256 checked) |
| `prepare.sh` | rsync → `target/lynx-myos-build`, install `lynx_cfg.h` / stubs |
| `build.sh` | Cross-compile → `target/lynx-<arch>-unknown-none` |

Thin wrappers: `scripts/fetch-lynx.sh`, `scripts/build-lynx.sh`.

## Config choice

`lynx_cfg.h` is **hand-written** for freestanding myos (not host `./configure`),
same approach as `packages/vim/config.h` and `ports/curl/config-myos.h`.

## SSL

Lynx's own OpenSSL path (`USE_OPENSSL_INCL`, `HTTLS.c`) over **LibreSSL**
(`ports/libressl`), the TLS library of curl, nginx and `user/tls` as well:
TLS 1.2 and 1.3, the peer verified against the Mozilla CA bundle curl ships
as `/lib/cacert.pem` (`SSL_CERT_FILE_DEFAULT`).

Default config is `packages/lynx/lynx.cfg`, packed as `/lib/lynx.cfg` (myos has no `/etc` mount; cpio only registers `bin/*` and `lib/*`). Lynx is built with `LYNX_CFG_FILE=/lib/lynx.cfg` and exits if that file is missing.

## Reuse (no duplication)

- Sockets: `toolchain/newlib/libgloss/myos/socket.c` over `/net` (no new stubs)
- DNS: libgloss `gethostbyname` / `getaddrinfo` (`netdb.c`) — same `/net/udp` path to the DHCP lease's DNS servers (`/net/ndb`) as curl and `user/lib/src/dns.rs`. Lynx is IPv4-only (`ENABLE_IPV6` off), so `HTTCP.c` calls `gethostbyname`; both symbols share `resolve_a`.
- Screen: `packages/ncurses`
- TLS: `ports/libressl`

`lynx www.google.com` paints **Looking up … first** then `HTCheckForInterrupt` (`select` on stdin, 0 timeout). That must not block in `LYgetch` — see libgloss `pollselect.c` (tty POLLIN is not always-ready).

## Image path

Packed into initramfs as `/bin/custom/lynx`. Missing ELF is a hard error when
required (same pattern as vim).

## Try

```sh
./packages/lynx/build.sh
ls -lh target/lynx-*-unknown-none
```
