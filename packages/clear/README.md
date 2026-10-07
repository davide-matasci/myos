# clear for myos

The `clear` program from **ncurses** (the same upstream 6.5 the `ncurses`
package pins): it writes the terminal's clear-screen capability to stdout.

Not shipped in the boot image; `get-myos clear` installs it as
`/bin/custom/clear`.

## How it builds

`clear` is three files of the ncurses source tree
(`progs/{clear,clear_cmd,tty_settings}.c`), linked against the static
`libncurses.a` the `ncurses` package already cross-builds. `build.sh` runs
`ncurses/build.sh` first (which fetches/prepares `target/ncurses-src` and
builds the per-arch library), then compiles and links `clear` for x86_64,
aarch64 and riscv64. There is no separate version pin: the source and the
library come from `ncurses`, so `clear`'s cache key folds in
`myos_ncurses_version_hash`.

## Runtime

ncurses here has no terminfo database; it reads termcap and has `linux`,
`vt100`, `ansi` and `dumb` compiled in as fallbacks. getty sets
`TERM=linux` and `TERMCAP=/lib/termcap` (`ports/termcap`), whose `linux`
entry is `cl=\E[H\E[J` — cursor home, then erase to end of display, the CSI
the framebuffer console implements. The `linux` entry has no `E3`
(clear-scrollback) capability, so `clear` emits only the two sequences.
