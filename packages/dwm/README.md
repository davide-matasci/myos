# dwm for myos

[dwm](https://dwm.suckless.org/), suckless's tiling window manager, on the
X server of the `tinyx` package. It is driven from the keyboard, which suits
an X server without a mouse.

```sh
get-myos x11-libs tinyx dwm
Xfbdev :0 -br &
DISPLAY=:0 dwm &
```

The stock configuration (`config.def.h`): Alt is the modifier, Alt+B toggles
the bar, Alt+J/K move the focus, Alt+Return zooms, Alt+Shift+C closes a
window, Alt+1..9 switch tags, Alt+Shift+Q quits. Alt+Shift+Return starts
`st` and Alt+P `dmenu_run`, neither of which is ported yet.

## What it is built from

| Component | Pin | |
|-----------|-----|-|
| dwm | `versions.env` (release tarball) | MIT/X Consortium |
| the X libraries (`packages/x11-libs`) | their pins | |

Static, through the same cross `cc` as x11-libs (`myos_write_cross_cc` in
`scripts/myos-c-userspace-lib.sh`), with dwm's own compiler flags and
without Xinerama.

## The patch

`core-fonts.myos.patch`:

- `drw.c` draws text with core X fonts (`XLoadQueryFont`, `XDrawString`)
  instead of Xft, which would need freetype, fontconfig and font files; the
  font is the server's built-in `fixed`. UTF-8 is drawn as Latin-1, with
  `?` for anything past it, and text that does not fit ends in `...`.
- `dwm.c` reaps its children in a `SIGCHLD` handler (as dwm 6.2 did) when
  the libc has no `SA_NOCLDWAIT`, which myos lacks.
- `config.def.h` names the `fixed` font.

`riscv64-sf-arith.c` holds the single-precision soft-float helpers riscv64
needs (dwm's `mfact` is a float), as `packages/git` has them.

## The test

`test.sh` (full mode, after the install): `Xfbdev :0 -br` and `dwm`; the
framebuffer's top left pixel turns the selected tag's `#005577`, after the
Alt+B the host types through the QEMU monitor the bar is gone (the black
root shows), and a second Alt+B brings it back. `dwm_smoke` waits for the
server's socket and reads the pixel.

## Not yet

- `st` and `dmenu`, the programs the default key bindings start.
- Xinerama (one screen anyway) and Xft fonts.
