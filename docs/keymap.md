# Loadable keyboard keymaps

The keyboard drivers decode hardware events to stable keycodes; the
characters come from a keymap loaded at runtime. myos has no compiled-in
ASCII table, so until a map is loaded the keyboard produces no console
input. Serial input (`ttyS0`, `docs/tty.md`) does not depend on it.

## The map at boot

`user/init` loads `/lib/kbd/ch.map` before getty (`DEFAULT_KEYMAP` in
`user/init/src/main.rs`; point it at `/lib/kbd/us.map` to make US the
default). It prints `[ OK ] keymap ch`, or `[ FAIL ] keymap {ctl|load} ch`
and falls back to `us.map`, so the keyboard is never left without a map.
The maps are in `modules/console/keymaps/` and ship under `/lib/kbd/`.

## Loading a map

The map is a line of the console's control file (`docs/tty.md`):

```
$ grep keymap /dev/console/ctl
keymap /lib/kbd/ch.map
$ echo 'keymap /lib/kbd/us.map' > /dev/console/ctl
```

- The kernel reads the map text from that path, in the writer's view of
  the tree, at most 8 KiB.
- Reading gives the loaded map's path, or `keymap none`.
- A file that cannot be read, is too long or does not parse fails the
  write and leaves the loaded map in place.
- A pty's ctl refuses the line: the map is the console's.

## Map file format

```
keycode <N> = <base> <Shift> <AltGr> <Shift+AltGr>
```

- `<N>` is decimal or `0xNN`, a PS/2 set-1 make code.
- A level is a printable byte, `none`, `ENTER`/`\n`, `TAB`/`\t`,
  `BKSP`/`\b`, `SPACE`, or an explicit byte `\xNN`/`0xNN`.
- `#` starts a comment.

```
keycode 0x18 = o O none none
keycode 0x08 = 7 / | none
```

`ch.map` is ISO Swiss German QWERTZ, with AltGr giving `@ # | \ [ ] { }`.
Accented letters are Latin-1 bytes (`\xFC` = ü): maps are not UTF-8,
since the tty path is a single-byte stream.

## Keycodes and modifiers

The PS/2 driver produces set-1 make codes
(`modules/console/ps2-scancode`); virtio-input on aarch64 and riscv64
produces Linux `KEY_*` codes, which have the same numbers for the
alphanumerics. `/dev/console/kbd` reports Linux `KEY_*` numbers on every
arch (`docs/tty.md`).

The modifiers are Shift, AltGr (Right Alt), Ctrl and Caps Lock. Caps Lock
flips Shift for letter keys; QEMU's VNC server needs it, since it sends an
uppercase letter pasted without Shift as Caps Lock around the letter. The
Caps Lock light is not driven.

## Reserved keys

`modules/console/src/kbd.rs` handles a few keys before the keymap, so they
work with any map: Esc (`0x1b`), the arrows (`ESC [ A` to `ESC [ D`) and
Ctrl+letter (`ch & 0x1f`, so Ctrl+C is `0x03` and reaches `ISIG`).

In cooked mode the kernel swallows complete escape sequences, so arrows
never leak `[` and letters into a canonical command line; a lone Esc is
dropped. In raw mode (`ICANON` cleared) the bytes pass untouched. The
shell, oksh, reads its prompt in raw mode with its Emacs line editor,
which maps the arrows and Home/End to history and cursor movement; the
tty is back in cooked mode while a foreground child runs, so `^C` still
reaches it.

## Test

`keymap` (`user/tests/kernel.sh`, tag `console`).
