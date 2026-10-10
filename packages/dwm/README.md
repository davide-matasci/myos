# dwm for myos

[dwm](https://dwm.suckless.org/), suckless's tiling window manager, on the
X server of the `tinyx` package. It is driven from the keyboard, which suits
an X server without a mouse.

```sh
get-myos tinyx dwm            # st, dmenu, the Xft stack and the fonts come with it
run-myos tinyx,dwm:startx
```

`startx` (the tinyx package) runs the server and dwm, and stops the server
when dwm quits (Alt+Shift+Q); the two apps share one view
(`docs/packages.md`), in which dwm finds st and dmenu, the apps it needs.

The stock configuration (`config.def.h`): Alt is the modifier, Alt+B toggles
the bar, Alt+J/K move the focus, Alt+Return zooms, Alt+Shift+C closes a
window, Alt+1..9 switch tags, Alt+Shift+Q quits. Alt+Shift+Return starts
`st` (the `st` package) and Alt+P `dmenu_run` (the `dmenu` package). The bar's font
is fontconfig's `monospace` (DejaVu Sans Mono, the `x11-fonts` package).

## What it is built from

| Component | Pin | |
|-----------|-----|-|
| dwm | `versions.env` (release tarball) | MIT/X Consortium |
| the X and font libraries (`packages/x11-xft`, `packages/x11-libs`) | their pins | |

Upstream dwm, unpatched: Xft draws its text and `SA_NOCLDWAIT` reaps its
children. Static, through the same cross `cc` as the X libraries
(`myos_write_cross_cc` in `scripts/myos-c-userspace-lib.sh`), with dwm's own
compiler flags and without Xinerama.

## The test

`test.sh` (full mode, after the install): `Xfbdev :0 -br` and `dwm`; the
framebuffer's top left pixel turns the selected tag's `#005577`, after the
Alt+B the host types through the QEMU monitor the bar is gone (the black
root shows), and a second Alt+B brings it back. `dwm_smoke` waits for the
server's socket and reads the pixel; when the bar does not come it reports
the unix conversations, whether the server answers a new client and what
the server itself has drawn.

## Not yet

- Xinerama (one screen anyway).
