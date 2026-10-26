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

`glib-myos.myos.patch` gives myos `g_unix_fd_query_path`, through
`/proc/self/fd` as on Linux (`__myos__`, which the build defines).

The app is `gio` (GIO's command: `run-myos glib:gio cat FILE`) and
`glib-smoke` (`smoke.c`), the package's test: the main loop with a
thread feeding it, threads over an async queue, a GObject's property and
signal, `GRegex`, GIO's files, a `GSocket` echo over loopback, `g_spawn`
and `g_convert`.

GLib, GNU libiconv and proxy-libintl are LGPL (`THIRD_PARTY_NOTICES.md`);
the programs link them statically.
