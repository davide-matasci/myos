# libsoup-smoke

The network libraries' program, `libsoup-smoke` (`smoke.c`, the
package's test), against mlibc, linked from `packages/libsoup`'s prefix:
libsoup GETs `/hello.txt` from the launcher's test server
(`src/test_www.rs`) over HTTP, and over HTTPS with the test CA as the
session's TLS database; with the image's bundle instead, the handshake
fails on the server's certificate. libxml2 parses a document and libpsl
finds a registrable domain.

```sh
get-myos libsoup-smoke
run-myos libsoup-smoke:libsoup-smoke
```

It is a package of its own, not libsoup's, so that a change to what it
links and the libraries never compile against (the libc's C, GIO's TLS
backend, LibreSSL) relinks it and rebuilds no library (`docs/ports.md`
"Cache keys").

libsoup and glib-networking are LGPL, glib-networking with an OpenSSL
linking exception that covers LibreSSL; libpsl's built-in list is MPL-2.0
(`THIRD_PARTY_NOTICES.md`). The program links them statically.
