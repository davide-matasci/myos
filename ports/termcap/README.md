# myos termcap

Minimal `/lib/termcap` describing the framebuffer ANSI/VT100 CSI subset
implemented in `kernel/src/framebuffer.rs` (`linux`, `ansi`, `vt100`, `dumb`).

## Path

- Source: `ports/termcap/termcap`
- Initramfs entry: `lib/termcap` → VFS `/lib/termcap` (via libfs)
- Getty sets `TERMCAP=/lib/termcap` (and `TERM=linux`) so ncurses `tgetent`
  loads the `linux` entry without a full terminfo database.

Column/row counts in the file are defaults; vim uses `TIOCGWINSZ`, which
reports the real framebuffer character-cell size.
