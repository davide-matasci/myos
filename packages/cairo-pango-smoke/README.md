# cairo-pango-smoke

The text and drawing libraries' program, `cairo-pango-smoke` (`smoke.c`,
the package's test, with x11-xft's `fonts.conf` and the x11-fonts
package), against mlibc, linked from `packages/cairo-pango`'s prefix:
fribidi reorders a mixed line, HarfBuzz shapes a word, pango lays out a
line that cairo draws into a PNG, which gdk-pixbuf loads back (and as a
JPEG through memory); then, on Xfbdev, cairo's Xlib surface fills a
rectangle that `XGetImage` reads back.

```sh
get-myos tinyx x11-xft x11-fonts cairo-pango-smoke
run-myos tinyx:Xfbdev :0 -br &
run-myos cairo-pango-smoke,x11-xft,x11-fonts:cairo-pango-smoke
```

It is a package of its own, not cairo-pango's, so that a change to what it
links and the libraries never compile against, the libc's C first,
relinks it and rebuilds no library (`docs/ports.md` "Cache keys").

fribidi, cairo, pango and gdk-pixbuf are LGPL (`THIRD_PARTY_NOTICES.md`);
the program links them statically.
