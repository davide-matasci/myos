# dmenu for myos

[dmenu](https://tools.suckless.org/dmenu/), suckless's dynamic menu, on the
X server of the `tinyx` package: it reads lines on its input, shows them in
a bar at the top of the screen, narrows them as you type and prints the one
you pick. dwm's Alt+P runs its `dmenu_run`, which lists the programs of
`PATH` and runs the one you pick.

```sh
get-myos x11-libs tinyx x11-xft x11-fonts dwm dmenu
startx                  # then Alt+P in dwm
```

The package installs `dmenu`, `stest` (the file tests `dmenu_path` filters
`PATH` with) and the upstream scripts `dmenu_run` and `dmenu_path`, shell
scripts the kernel runs through their `#!/bin/sh` line. `dmenu_path` caches
the program list in `$XDG_CACHE_HOME/dmenu_run` (`$HOME/.cache` without it,
`/.cache` for root) and lists it again when a `PATH` directory changes. The
stock configuration (`config.def.h`): the font is fontconfig's `monospace`
(DejaVu Sans Mono, the `x11-fonts` package), the selected item `#005577`.

## What it is built from

| Component | Pin | |
|-----------|-----|-|
| dmenu | `versions.env` (release tarball) | MIT/X Consortium |
| the X and font libraries (`packages/x11-xft`, `packages/x11-libs`) | their pins | |

Upstream dmenu, unpatched, without Xinerama. `myos_compat.h` declares
`getline` and `nanosleep`, which libgloss has and newlib's headers leave
out. Static, through the same cross `cc` as the X libraries
(`myos_write_cross_cc` in `scripts/myos-c-userspace-lib.sh`), with dmenu's
own compiler flags.

## The test

`test.sh` (full mode, after the install), on `Xfbdev :0 -br` without a
window manager (dmenu grabs the keyboard):

- `dmenu`: `alpha`, `beta` and `gamma` on its input; `dmenu_smoke` waits for
  the server's socket and for the bar (the selected item's `#005577` along
  the screen's top), the host types `b` and Return through the QEMU monitor,
  and dmenu prints `beta`.
- `dmenu_run`: `dmenu_path` lists a `#!` script put in `/bin/custom` (and
  `dmenu`, `sh`); `dmenu_run` shows the bar, the host types `z`, `z` and
  Return, and the shell it pipes the pick into runs the script.

## Not yet

- Programs in a directory added to `PATH`: the shell (oksh) sets its own
  `PATH` whatever it inherits (`TODO.md`), so `dmenu_path` lists the default
  directories, where packages install.
- Xinerama (one screen anyway).
