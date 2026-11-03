# X development files for myos

What building an X program on myos itself takes, beside `tcc` and `make`:
the headers and static libraries of the X client libraries
(`packages/x11-libs`, its newlib build) at `/lib/x11`, the prefix they were
built for. tcc searches `/lib/x11/include` and `/lib/x11/lib`
(`packages/tcc/config.h`), so a Makefile needs no `-I` or `-L`; tcc is
`cc` too, make's default `$(CC)`.

```sh
get-myos tcc make git x11-dev
get-myos --global tcc make git x11-dev    # on PATH, and /lib/x11 for every process
git clone https://github.com/torrinfail/dwmblocks && cd dwmblocks
make && ./dwmblocks &                     # in st, under dwm
```

Without `--global`, one view: `run-myos tcc,make,x11-dev:make`.

## What it holds

- The headers: xorgproto (`X11/X.h`, `Xproto.h`, ...), Xlib, xcb, Xau and
  the extensions' (`X11/extensions/`).
- The static libraries: libX11, libxcb, libXau, libXext, libXrender,
  libXfixes, libXi, libXrandr, libXdamage, libXcomposite, libXcursor,
  libXinerama. `libX11.a` has libxcb and libXau merged into it: a static
  Xlib needs both, and a Makefile written for a shared one passes only
  `-lX11`.

The runtime data Xlib reads (its error database, the locale files) is
`x11-libs`'s, which get-myos installs with it.

## The test

`test.sh` (full mode, after the install): `make` builds a client from a
Makefile with `$(CC) -o xname xname.c -lX11` (cc, tcc), the client names
the root window on `Xfbdev :0 -br`, as dwmblocks sets dwm's status, and
`xprop -root WM_NAME` (`x11-apps`) prints the name back.

## Not yet

- The Xft font stack (`packages/x11-xft`: libXft, fontconfig, FreeType),
  which dwm, st and dmenu draw their text with: added here when wanted.
- dwmblocks' signals: newlib has five real-time signals, `SIGRTMIN` 27 to
  `SIGRTMAX` 31 (issue #738), so a block refreshed by a signal must use 1
  to 4; the others still update on their interval.
