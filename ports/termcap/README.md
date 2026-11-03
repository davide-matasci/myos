# myos termcap

Minimal `/lib/termcap` describing the framebuffer ANSI/VT100 CSI subset
implemented in `kernel/src/framebuffer.rs` (`linux`, `ansi`, `vt100`, `dumb`),
and `st` / `st-256color` for the terminal of the `st` package (converted
from its `st.info`; st sets `TERM=st-256color`).

## Path

- Source: `ports/termcap/termcap`
- Initramfs entry: `lib/termcap` → `/lib/termcap`
- Getty sets `TERMCAP=/lib/termcap` (and `TERM=linux`) so ncurses `tgetent`
  loads the `linux` entry without a full terminfo database.
- `linux` and `ansi` say 8 colors (`Co#8`, with `AF`/`AB` for SGR 30-37
  and 40-47, which the console draws): vim colors its syntax only on a
  terminal whose entry gives a color count.

Column/row counts in the file are defaults; vim uses `TIOCGWINSZ`, which
reports the real framebuffer character-cell size.
