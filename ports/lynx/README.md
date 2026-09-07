# Lynx for myos

Upstream **Lynx 2.9.3** (fetched at build; not vendored). Cross-built with
newlib + myos libgloss **userspace BSD sockets** (`/net`) + static ncurses +
**mbedtls** for HTTPS.

## Layout

| Script | Role |
|--------|------|
| `fetch.sh` | Download pinned tarball → `target/lynx-src` (SHA-256 checked) |
| `prepare.sh` | rsync → `target/lynx-myos-build`, install `lynx_cfg.h` / tidy_tls / stubs |
| `build.sh` | Cross-compile → `target/lynx-<arch>-unknown-none` |

Thin wrappers: `scripts/fetch-lynx.sh`, `scripts/build-lynx.sh`.

## Config choice

`lynx_cfg.h` is **hand-written** for freestanding myos (not host `./configure`),
same approach as `ports/vim/config.h` and `ports/curl/config-myos.h`.

## SSL (not a workaround)

Lynx speaks OpenSSL or GnuTLS-via-`tidy_tls`. There is no in-tree OpenSSL and
we must not add a second TLS stack. We keep lynx’s existing **`USE_GNUTLS_INCL`
+ tidy_tls** integration surface and implement `ports/lynx/tidy_tls.{h,c}` on
**ports/mbedtls** — the same library curl and `user/tls` already use. CA bundle
path: `/lib/cacert.pem` (shipped for curl).

Default config is `ports/lynx/lynx.cfg`, packed as `/etc/lynx.cfg` (Lynx exits if that path is missing).

## Reuse (no duplication)

- Sockets: `toolchain/newlib/libgloss/myos/socket.c` over `/net` (no new stubs)
- Screen: `ports/ncurses`
- TLS: `ports/mbedtls` (+ tidy_tls glue only)

## Image path

Packed into initramfs as `/bin/custom/lynx`. Missing ELF is a hard error when
required (same pattern as vim).

## Try

```sh
./ports/lynx/build.sh
ls -lh target/lynx-*-unknown-none
```
