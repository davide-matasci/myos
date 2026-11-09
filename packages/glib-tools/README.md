# glib-tools

GLib's programs on myos, against mlibc: `gio` (GIO's command: `run-myos
glib-tools:gio cat FILE`) and `glib-smoke` (`smoke.c`), the package's
test: the main loop with a thread feeding it, threads over an async
queue, a GObject's property and signal, `GRegex`, GIO's files, a
`GSocket` echo over loopback, `g_spawn` and `g_convert`.

```sh
get-myos glib-tools
run-myos glib-tools:gio cat /etc/policy
```

Both are linked from `packages/glib`'s prefix (gio from the objects GLib's
build leaves in `lib/gio-tool`). They are a package of their own, not
glib's, so that a change to what they link and GLib never compiles
against, the libc's C first, relinks them and rebuilds no library
(`docs/ports.md` "Cache keys").

GLib, GNU libiconv and proxy-libintl are LGPL (`THIRD_PARTY_NOTICES.md`);
the programs link them statically.
