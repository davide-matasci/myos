# TinyX's Xfbdev for myos

The X server: [TinyX](https://github.com/tinycorelinux/tinyx), Tiny Core's
continuation of X.Org's kdrive servers (X.Org 1.2.0 with fixes), its `Xfbdev`
built for myos. Core protocol, no mouse, no XKB, no GL; the screen is
`/dev/fb` (`docs/fb.md`), the keyboard `/dev/console/kbd` (`docs/tty.md`).

```sh
get-myos tinyx
startx some-x-client        # dwm when no client is named
```

The server takes the screen and the keyboard from the console while it runs
and gives them back when it exits (Ctrl+Alt+Backspace ends it; serial input
still reaches the shell meanwhile). That is why a session starts in one
command: what is typed after `Xfbdev :0 &` goes to the server, not to the
shell. `startx [program [arg...]] [-- server-arg...]` (`startx.c`) starts
`Xfbdev :0 -br` with SIGUSR1 ignored, waits for the signal the server sends
its parent once it listens (xinit's handshake), runs the program with
`DISPLAY=:0`, and stops the server when the program exits.

## What it is built from

| Component | Pin | |
|-----------|-----|-|
| TinyX | `TINYX_REV` (a commit of its master branch) | GPL-3.0 (its changes; the X.Org base is MIT) |
| libfontenc, libXfont 1.x | `versions.env` | MIT; libXfont 1.x because TinyX predates libXfont2 |
| libXdmcp | `versions.env` | only its header: TinyX's `os/` includes it with XDMCP off |
| zlib (`ports/zlib`), the X libraries (`packages/x11-libs`) | their pins | |

All static, through the same cross `cc` as x11-libs
(`myos_write_cross_cc` in `scripts/myos-c-userspace-lib.sh`). TinyX's git
tree has no `configure`; `build.sh` runs `autoreconf` after the patches.
Fonts: libXfont's built-in `fixed` and `cursor` (the font path is
`built-ins`), so nothing else needs installing.

## The patches

| Patch | What |
|-------|------|
| `kdrive-myos.myos.patch` | `--with-kdrive-os=myos`: `kdrive/myos` (the OS layer: nothing to switch, no mouse; the keyboard from `/dev/console/kbd`, the keymap from the console's keymap file, US when there is none) and `kdrive/fbdev/myosfb.c` (the framebuffer ioctls `fbdev.c` makes, answered from `/dev/fb/ctl`; writing `graphics` there takes the screen, the server's exit closes it and the console takes it back) |
| `no-shm-xtest.myos.patch` | MIT-SHM (no SysV shared memory) and XTEST are not built |
| `sync-headers.myos.patch` | SYNC's constants from xorgproto's `syncconst.h` (libXext's client `sync.h` carried them in 2007) |
| `arches.myos.patch` | `servermd.h` entries for AArch64 and RISC-V 64 |
| `lock-without-link.myos.patch` | the `/tmp/.X0-lock` file without `link()` (myos has no hard links) |
| `builtin-fonts.myos.patch` | `KDRIVESERVER` in `dix-config.h`, which registers libXfont's built-in fonts |

`include/` and `myos_compat.h` hold the libc bits TinyX expects
(`linux/fb.h` for `fbdev.c`, `net/if.h`, `lstat`); libgloss's
`getservbyname` finds nothing, so xtrans uses the port number.

## The test

`test.sh` (full mode, after the install): `startx /bin/etc/tinyx_smoke`;
`tinyx_smoke` checks that the screen is `/dev/fb`'s size, maps a red window over it and
reads the framebuffer back, and receives the Shift+A the host types through
the QEMU monitor as keycode 38, "A"; when it exits startx stops the server,
and `/dev/fb/ctl` says `text` again.

## Not yet

- A mouse (`kdrive/myos` has none; the core pointer never moves).
- Rotation and modes: one framebuffer, at the mode the bootloader set.
- Fonts beyond the built-in two (`/lib/X11/fonts`).
- Clients beyond dwm, st and dmenu (issue #273).
