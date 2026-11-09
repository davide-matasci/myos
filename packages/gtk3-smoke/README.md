# gtk3-smoke

GTK 3's program, `gtk3-smoke` (`smoke.c`, the package's test, with TinyX,
x11-xft's `fonts.conf` and the x11-fonts package), against mlibc, linked
from `packages/gtk3`'s prefix: on Xfbdev a window holds a label, a button
and an entry; once it has drawn, the button is clicked, text goes into
the entry and is read back, and the label's pixels are read from the
window.

```sh
get-myos tinyx x11-xft x11-fonts gtk3-smoke
run-myos tinyx:Xfbdev :0 -br &
run-myos gtk3-smoke,x11-xft,x11-fonts:gtk3-smoke
```

It is a package of its own, not gtk3's, so that a change to what it links
and GTK never compiles against, the libc's C first, relinks it and
rebuilds no library (`docs/ports.md` "Cache keys").

GTK and ATK are LGPL (`THIRD_PARTY_NOTICES.md`); the program links them
statically.
