# glib

[GLib](https://gitlab.gnome.org/GNOME/glib) (GLib, GObject, GIO, GModule,
GThread) as static libraries against mlibc, the base of WebKitGTK's
stack (issue #616). One prefix per arch, `target/glib-mlibc-<arch>`
(`lib/` with the archives and their pkg-config files, `include/`), holds
GLib and what it links that mlibc lacks:

- zlib, `ports/zlib`'s mlibc build;
- PCRE2, `packages/pcre2`'s mlibc build, for `GRegex`;
- libffi, for GObject's generic marshaller (a signal without a
  marshaller of its own); its closures map their trampolines writable and
  executable;
- GNU libiconv, for `g_convert` and the filename and locale charsets
  (mlibc has no `iconv`);
- proxy-libintl, GLib's `gettext` when the libc has none: it returns the
  message (no translations, `-Dnls=disabled`), built without its `dlopen`
  of a real libintl.

`build.sh` builds libffi and libiconv with their own `configure`, then
GLib with meson, cross, both with the cross cc of `myos_write_cross_cc`
(clang for the bare target does not link for every arch; that cc runs
ld.lld). meson runs nothing it builds, so GLib's run checks take their
defaults: its own printf (gnulib's) for the C99 and positional ones.
Off: introspection, the tests, libmount, SELinux, xattrs, sysprof, dtrace.
GIO's file monitor is its polling one (no inotify).

The libraries over GLib compile their resources while they build
(gdk-pixbuf, GTK), with a `glib-compile-resources` that runs on the build
host: `build.sh` also builds GLib natively, with the host's compiler, over
host builds of libffi, PCRE2 and zlib from the same pins, and keeps that
tool in `target/glib-host/bin` (the CI image has no GLib of its own),
with GLib's Python generators (`glib-mkenums`, `glib-genmarshal`) as that
build configures them.

`glib-myos.myos.patch` gives myos `g_unix_fd_query_path`, through
`/proc/self/fd` as on Linux (`__myos__`, which the build defines).

A library package: nothing to install. Its programs, `gio` (GIO's
command) and `glib-smoke` (the test), are the glib-tools package's
(`packages/glib-tools`), linked from this prefix: a library port ships
no program, so that its cache key can leave out what it links and never
compiles against, the libc's C first (`docs/ports.md` "Cache keys"). gio's
objects are left in `lib/gio-tool` for that link.

GLib, GNU libiconv and proxy-libintl are LGPL (`THIRD_PARTY_NOTICES.md`);
glib-tools' programs link them statically.
