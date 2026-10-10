# X applications for myos

Small X programs on the X server of the `tinyx` package. Today that is
[xev](https://gitlab.freedesktop.org/xorg/app/xev), which opens a window (or
watches the root with `-root`, or another window with `-id`) and prints
every event it gets: keys with their keycode, keysym and the text they
type, focus, exposure, configure and RandR screen changes.

```sh
get-myos tinyx dwm x11-apps
run-myos tinyx,dwm,x11-apps:startx      # then, in st: xev
run-myos x11-apps -root -event keyboard # or, on a server without a window manager
```

## What it is built from

| Component | Pin | |
|-----------|-----|-|
| xev | `versions.env` (release tarball) | MIT/X Consortium |
| libXext, libXrandr | `versions.env` (release tarballs) | MIT-Open-Group / HPND-style |
| the X libraries (`packages/x11-xft`, `packages/x11-libs`) | their pins | |

Upstream sources, unpatched, each with its own autoconf `configure` in
cross mode (`build.sh`), static, on top of a copy of the font stack's
stage (libX11, xcb and libXrender, which libXrandr builds on):
`target/x11-apps-<arch>`. `malloc(0)` is not `NULL` on newlib, as for
libX11.

## The test

`test.sh` (full mode, after the install), on `Xfbdev :0 -br` without a
window manager: `xev -root -event keyboard` (with no other window the
keyboard's focus is the root), the host types `x` through the QEMU
monitor, and xev prints a `KeyPress` and a `KeyRelease` with keysym
`0x78, x`.

## Not yet

- `xsetroot` and the other xorg apps: added here when wanted.
- Pointer events: the server has no mouse (issue #302).
