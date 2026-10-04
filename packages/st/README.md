# st for myos

[st](https://st.suckless.org/), suckless's simple terminal, on the X server
of the `tinyx` package: the terminal dwm's Alt+Shift+Return starts.

```sh
get-myos x11-libs tinyx x11-xft x11-fonts dwm st
startx                  # then Alt+Shift+Return in dwm
startx st               # or st alone, without a window manager
```

The stock configuration (`config.def.h`): the font asked for is
`Liberation Mono`, and fontconfig, which has no such font, gives the closest
it has, DejaVu Sans Mono (the `x11-fonts` package); Ctrl+Shift+PageUp and
PageDown zoom. st runs `$SHELL` (oksh) on a
pty (`/dev/pts`, `docs/tty.md`) with `TERM=st-256color`, which
`/lib/termcap` describes (`ports/termcap`).

## What it is built from

| Component | Pin | |
|-----------|-----|-|
| st | `versions.env` (release tarball) | MIT/X Consortium |
| the X and font libraries (`packages/x11-xft`, `packages/x11-libs`) | their pins | |

Upstream st with one patch, `st.c.myos.patch`: st unsets `TERMCAP` for its
shell, which on myos is the path of `/lib/termcap`, not an entry of the
outer terminal, and ncurses (whose default path does not include it) would
find no description of `st-256color`. `myos_compat.h` declares what newlib's
headers lack (`openpty` from libgloss's `<pty.h>`, which st includes only on
Linux, `clock_gettime`, `_POSIX_ARG_MAX`). Static, through the same cross
`cc` as the X libraries (`myos_write_cross_cc` in
`scripts/myos-c-userspace-lib.sh`), with st's own compiler flags.

## The test

`test.sh` (full mode, after the install): `Xfbdev :0 -br` and, with no
window manager, st running a shell that prints a line, then reads one.
`st_smoke` waits for the server's socket and for st's window, gives it the
keyboard's focus as a window manager would (with no pointer device the
server's focus does not follow a window mapped under the pointer), then
waits for lit pixels in the screen's top left corner (st's first line). The host types `o`, `k`
and Return through the QEMU monitor, the shell saves the line with `$TERM`
(`ok st-256color`) and exits, and st exits with it, status 0; when the line
does not come, `st_smoke probe` reports the pointer, the focus and the
windows.

## Not yet

- The mouse: no selection, no pasting with it (the server has no pointer).
- Scrollback (upstream st has none; its patches add it).
