# webkitgtk-smoke

WebKitGTK's program: a GTK window with a web view, and WebKit's web and
network processes, in one binary (`smoke.c`, against mlibc), linked from
`packages/webkitgtk`'s archive as surf is. `webkitgtk-smoke URL` loads
URL; with no URL, the launcher's test page.

```sh
get-myos tinyx x11-xft x11-fonts webkitgtk-smoke
run-myos tinyx:Xfbdev :0 -br &
run-myos webkitgtk-smoke,x11-xft,x11-fonts:webkitgtk-smoke https://example.org
```

It is a package of its own, not WebKit's, so that what it links and WebKit
never compiles against (GIO's TLS backend in `packages/libsoup`, and
LibreSSL) can change with a relink of this program and no WebKit build
(issue #664, `docs/ports.md` "Cache keys"). ICU's data is the webkitgtk
prefix's file, installed in the app as `/lib/webkitgtk/icudt79l.dat`.

## The test

`test.sh` (full mode, after the install, with TinyX, x11-xft's
`fonts.conf` and the x11-fonts package): on Xfbdev a web view loads the
launcher's test page (`src/test_www.rs`); once WebKit says it loaded and
its title came (the title may come after the load), what its script
wrote (asked through JavaScript) and its blue box on the screen (read
back from the window) are checked.

WebKit's WebCore, libgpg-error, libgcrypt and libtasn1 are LGPL
(`THIRD_PARTY_NOTICES.md`); the program links them statically.
