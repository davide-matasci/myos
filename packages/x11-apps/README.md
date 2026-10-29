# X applications for myos

Small X programs on the X server of the `tinyx` package:

- [xev](https://gitlab.freedesktop.org/xorg/app/xev), which opens a window
  (or watches the root with `-root`, or another window with `-id`) and
  prints every event it gets: keys with their keycode, keysym and the text
  they type, focus, exposure, configure and RandR screen changes;
- [xprop](https://gitlab.freedesktop.org/xorg/app/xprop), which prints a
  window's properties and sets them (`-set`): surf's address bar (Ctrl-g)
  and find (Ctrl-f) read and set its window's with it.

```sh
get-myos tinyx dwm x11-apps
run-myos tinyx,dwm,x11-apps:startx      # then, in st: xev
run-myos x11-apps -root -event keyboard # or, on a server without a window manager
run-myos x11-apps:xprop -root           # the root window's properties
```

## What it is built from

| Component | Pin | |
|-----------|-----|-|
| xev, xprop | `versions.env` (release tarballs) | MIT/X Consortium |
| the X libraries (`packages/x11-xft`, `packages/x11-libs`: libXext and libXrandr among them) | their pins | |

Upstream sources, unpatched, with each one's autoconf `configure` in cross
mode (`build.sh`), static, on top of a copy of the font stack's stage:
`target/x11-apps-<arch>`.

## The test

`test.sh` (full mode, after the install), on `Xfbdev :0 -br` without a
window manager: `xev -root -event keyboard` (with no other window the
keyboard's focus is the root), the host types `x` through the QEMU
monitor, and xev prints a `KeyPress` and a `KeyRelease` with keysym
`0x78, x`; then `xev -root -event mouse`, the host moves the boot's
`usb-mouse` and clicks its left button (`user/c/host.sh mouse`), and xev
prints a `MotionNotify` and the `ButtonPress` of button 1. Then
`xprop -root -f _MYOS_TEST 8u -set _MYOS_TEST "hello myos"` and
`xprop -root _MYOS_TEST`, which prints the UTF-8 string back.

## Not yet

- `xsetroot` and the other xorg apps: added here when wanted.
