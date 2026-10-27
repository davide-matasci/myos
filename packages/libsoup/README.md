# libsoup

The network libraries of WebKitGTK's stack (issue #616), static against
mlibc, in one prefix per arch, `target/libsoup-mlibc-<arch>` (`lib/` with
the archives and their pkg-config files, `include/`):

| Library | Notes |
|---------|-------|
| [libxml2](https://gitlab.gnome.org/GNOME/libxml2) | its test module left out of the build (`libxml2-testdso.myos.patch`: a shared one); no iconv (mlibc has none: its own UTF-8, UTF-16 and Latin-1), zlib, ICU or HTTP client |
| [libpsl](https://github.com/rockdaboot/libpsl) | the Public Suffix List of its release built in, for cookies' domains; no IDNA library |
| [nghttp2](https://nghttp2.org/) | HTTP/2, the library alone |
| [libsoup 3](https://gitlab.gnome.org/GNOME/libsoup) | the HTTP client of the `webkit2gtk-4.1` API; no GSSAPI, NTLM, brotli or zstd |
| [glib-networking](https://gitlab.gnome.org/GNOME/glib-networking) | GIO's TLS: its OpenSSL backend over LibreSSL's mlibc build (`ports/libressl`; `glib-networking-libressl.myos.patch` skips the two calls LibreSSL lacks: a shutdown error code of one OpenSSL release and the environment's signature algorithm list); no proxy modules |

`build.sh` builds them with meson, cross (`myos_mlibc_meson_cross` and
`myos_mlibc_meson_build`, as `packages/cairo-pango`), nghttp2 with its
autoconf build, over the mlibc builds of GLib, LibreSSL and SQLite (their
pkg-config files written for LibreSSL and SQLite, which install none).

GIO loads its TLS backend as a module, with `dlopen`, which myos lacks:
glib-networking's static library (`gioopenssl`, its own static build;
`glib-networking-static.myos.patch` leaves out the shared module) is linked
into the program, which registers it before its first TLS connection with
`g_io_openssl_load (NULL)`. The default certificates are LibreSSL's
default file, the image's CA bundle (`/lib/cacert.pem`).

The app is the package's smoke, `libsoup-smoke` (`smoke.c`, its test):
libsoup GETs `/hello.txt` from the launcher's test server
(`src/test_www.rs`) over HTTP, and over HTTPS with the test CA as the
session's TLS database; with the image's bundle instead, the handshake
fails on the server's certificate. libxml2 parses a document and libpsl
finds a registrable domain.

libsoup and glib-networking are LGPL, glib-networking with an OpenSSL
linking exception that covers LibreSSL; libpsl's built-in list is MPL-2.0
(`THIRD_PARTY_NOTICES.md`). The smoke links them statically.
