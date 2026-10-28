# surf for myos

[surf](https://surf.suckless.org/), suckless's web browser: one WebKit
view in an X window, driven by keys and by X properties, on the X server
of the `tinyx` package and the WebKitGTK of the `webkitgtk` package
(issue #616).

```sh
get-myos tinyx dwm surf        # surf brings x11-xft, x11-fonts, dmenu and x11-apps
run-myos tinyx,dwm:startx      # then, in dwm's terminal (Alt+Shift+Return):
run-myos surf http://example.org/
```

The keys are the stock configuration's (`config.def.h`, Ctrl the modifier):
Ctrl+G asks for an address with dmenu (the `SETPROP` script: `xprop` reads
the window's `_SURF_URI` into dmenu, and sets `_SURF_GO` to what you pick,
which surf loads), Ctrl+F finds in the page, Ctrl+H and Ctrl+L go back and
forward, Ctrl+J/K/Space/B scroll, Ctrl+R reloads, Ctrl+T shows the page's
certificate. surf keeps its cookies, cache, certificates, styles and
script under `$HOME/.surf`.

## What it is built from

| Component | Pin | |
|-----------|-----|-|
| surf | `versions.env` (git master: the last release, 2.1, is for the older `webkit2gtk-4.0` API, on libsoup 2) | MIT/X Consortium |
| WebKitGTK and its stack (`packages/webkitgtk` and the packages under it) | their pins | |

Against mlibc (`PORT_LIBC=mlibc`), as WebKitGTK is: surf's `surf.c` with
its stock `config.h`, compiled with its `config.mk` flags through the
mlibc cross `cc` (`myos_mlibc_meson_cross` in
`scripts/myos-c-userspace-lib.sh`) and linked with WebKit's archive. As
WebKit's own programs, surf is also WebKit's web and network processes:
its `main` is renamed `webkit_myos_program_main`, which the archive's
`main` calls when the process is neither (`packages/webkitgtk/README.md`).
ICU's data is the webkitgtk prefix's file, installed in the app as
`/lib/webkitgtk/icudt79l.dat`.

The patch (`surf.myos.patch`):

- no gcr (GNOME's certificate widgets, which would bring GnuPG's stack):
  Ctrl+T shows the certificate as its PEM text;
- no web extension: myos has no `dlopen`, so WebKit's web process loads
  none; surf's scroll keys, which ask its extension to scroll, scroll the
  page through JavaScript instead, by the same amounts.

## The test

`test.sh` (full mode, after the install), on `Xfbdev :0 -br` without a
window manager: surf (`-w` prints its window's id) opens the launcher's
test page (`src/test_www.rs`), and `surf_check` (an Xlib helper, built
here against mlibc) waits for the window's title, surf's `... | myos test
page` with the load done, and the page's blue box on the screen. Then
`xprop` sets the window's `_SURF_GO`, as Ctrl+G's script does, and
`surf_check` waits for its `_SURF_URI` to be the new address.

## Not yet

- Downloads and the external programs of `config.def.h`: `DOWNLOAD`
  runs curl in `st` (the `st` package, on `PATH` in dwm's terminal),
  `PLUMB` `xdg-open` and `VIDEOPLAY` `mpv`, which myos lacks.
- What WebKitGTK lacks here (media, WebGL, spell checking, the
  inspector's remote side): `packages/webkitgtk/README.md`.
