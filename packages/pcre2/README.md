# PCRE2 for myos

Upstream **PCRE2 10.49** (fetched at build; not vendored), built as the
static 8-bit library `target/pcre2-<arch>/lib/libpcre2-8.a` with Unicode
support and without the JIT. Nothing of it is in the image or a package:
it is linked into `packages/nginx` (regex `location`s, `rewrite`). The
same sources are built against mlibc as `target/pcre2-mlibc-<arch>`, for
GLib's `GRegex` (`packages/glib`).

The build uses upstream's `config.h.generic`, `pcre2.h.generic` and
`pcre2_chartables.c.dist` (what `NON-AUTOTOOLS-BUILD` describes) instead of
running `configure`, with the features set on the compiler's command line.
