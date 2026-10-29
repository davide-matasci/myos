# Loadable keyboard keymaps

myos does **not** ship a compiled-in US/CH ASCII table for live typing. The
keyboard drivers decode hardware events to **stable keycodes**: PS/2 set-1
make codes, or Linux `KEY_*` codes for the same positions. The characters
come from a keymap loaded at runtime. This page covers the maps, how to load
one, their file format and the keys handled outside them.

## The map at boot

- The kernel's keymap slot is **empty** after boot.
- Until a map is loaded, the keyboard produces **no** console ASCII.
- **Serial input is unchanged**, so CI and serial consoles keep working.
  The serial port is `ttyS0` (`docs/tty.md`).
- `user/init` loads the default map early, before getty:

```rust
const DEFAULT_KEYMAP: &[u8] = b"/lib/kbd/ch.map";
```

To make US the default, point that constant at `/lib/kbd/us.map`.

Both maps live in `modules/console/keymaps/` in the repo and ship in the
initramfs under `/lib/kbd/`.

`user/init` loads CH first:

- On success it prints `[ OK ] keymap ch`.
- When the console's control file cannot be opened, or refuses the line,
  it prints a distinct `[ FAIL ] keymap {ctl|load} ch`.
- It then falls back to `us.map`, so the PS/2 keyboard is never left
  without a map. Success prints `[ OK ] keymap us`.

## Loading a map: the console's control file

The console is a directory (`docs/tty.md`). Its state is the text of
`/dev/console/ctl`, and the keyboard map is one of its lines:

```
$ grep keymap /dev/console/ctl
keymap /lib/kbd/ch.map
$ echo 'keymap /lib/kbd/us.map' > /dev/console/ctl
```

- Writing `keymap PATH` makes the kernel read the map **text** in that
  file, in the writer's view of the tree, at most 8 KiB. The console
  module then installs it in place of the loaded one.
- Reading the ctl gives the file the loaded map came from, or
  `keymap none` before any map was loaded.
- The write fails, and leaves the previous map in place, when the file
  cannot be read, is too long or does not parse. For a parse error the
  console module also prints `[ FAIL ] keymap: <reason>`.
- A pty's ctl refuses the line: the map is the console's.

## Map file format (text)

Lines are:

```
keycode <N> = <base> <Shift> <AltGr> <Shift+AltGr>
```

- `<N>` is decimal or `0xNN` (keycode = PS/2 set-1 make).
- Each level token is one of:
  - a single printable byte (`a`, `/`, …);
  - `none`: empty (no character);
  - `ENTER` / `\n`, `TAB` / `\t`, `BKSP` / `\b`, `SPACE`;
  - `\xNN` or `0xNN`: an explicit byte (Latin-1 for CH letters).
- `#` starts a comment. Blank lines are ignored.

### Example

```
keycode 0x18 = o O none none
keycode 0x08 = 7 / | none
keycode 0x35 = - _ none none
```

## The Swiss German map

`ch.map` is ISO Swiss German QWERTZ:

- Physical `/` is **Shift+7**. This fixes the classic US-map `/` vs `_`
  mismatch.
- Physical `-`/`_` is the key US labels `/?`.
- **AltGr** (Right Alt) gives `@ # | \ [ ] { }` for daily shell use.
- Accented letters are Latin-1 bytes (`\xFC` = ü, …). v1 is **not UTF-8**,
  since the tty path is still a single-byte stream.

## Keycodes and modifiers

| Arch    | Hardware     | Keycode space                          |
|---------|--------------|----------------------------------------|
| x86_64  | PS/2 8042    | set-1 makes via `modules/console/ps2-scancode` |
| aarch64 | virtio-input | Linux `KEY_*` (same numbers for alphanumerics) |
| riscv64 | virtio-input | same as aarch64                        |

`/dev/console/kbd` reports keys by their Linux `KEY_*` number on every
arch, with the character this keymap gives them. The PS/2 driver converts
its codes (`docs/tty.md`).

The modifiers tracked are:

- **Shift**;
- **AltGr** (Right Alt);
- **Ctrl** (Left or Right);
- **Caps Lock**. It flips Shift for the keys whose plain character is a
  letter, and for `/dev/console/kbd`'s characters too. QEMU's VNC server
  needs it: an uppercase letter typed or pasted without Shift held (a
  VPS's web console pasting text) arrives as Caps Lock pressed around the
  letter. The keyboard's Caps Lock light is not driven.

## Reserved keys

A few keys are handled before the loadable keymap
(`modules/console/src/kbd.rs`). They work whichever map is loaded and are
not bound in `us.map` or `ch.map`:

| Key          | Keycode | Output bytes            |
|--------------|---------|-------------------------|
| Esc          | `0x01`  | `0x1b`                  |
| Up arrow     | `0x48`  | `ESC [ A` (3 bytes)     |
| Down arrow   | `0x50`  | `ESC [ B`               |
| Right arrow  | `0x4D`  | `ESC [ C`               |
| Left arrow   | `0x4B`  | `ESC [ D`               |

**Ctrl + letter** produces the control byte (`ch & 0x1f`). So `Ctrl+C`
gives `0x03`, which hits the kernel's `ISIG` ^C path in both cooked and raw
mode.

## Cooked and raw mode

- In **cooked** (canonical) mode the kernel swallows complete ESC/CSI
  sequences (arrows, function keys), so `[` and letters never leak into a
  plain, canonical command line. A lone Esc is dropped, as bash does.
  Cooked mode is what non-editing readers see.
- In **raw** mode (`ICANON` cleared) the kernel delivers the real ESC bytes
  untouched. This is how TUIs like `vim` and the shell's line editor get
  their keys.

## oksh: Emacs line editing on the raw console

The interactive shell (`ports/oksh`) does **not** read cooked canonical
lines. At the `$` prompt it puts its tty into raw/cbreak mode: it clears
`ICANON`\|`ECHO` and keeps `ISIG`, through `tcsetattr` (the kernel's
`TCSETS`). It then runs its bundled **Emacs** line editor (`emacs.c`) over
the raw bytes.

In raw mode the kernel delivers the `ESC [ A/B/C/D` bytes the keyboard
drivers produce directly, with no cooked swallowing, and oksh maps them:

| Key          | Bytes       | Emacs binding     | Effect        |
|--------------|-------------|-------------------|---------------|
| Up arrow     | `ESC [ A`   | `x_prev_com`      | history Up    |
| Down arrow   | `ESC [ B`   | `x_next_com`      | history Down  |
| Right arrow  | `ESC [ C`   | `x_mv_forw`       | cursor right  |
| Left arrow   | `ESC [ D`   | `x_mv_back`       | cursor left   |
| Home / End   | `ESC [ H/F` | `x_mv_begin/end`  | line start/end |

Raw mode turns off the kernel's echo, so oksh's editor owns echo and redraw
(`x_zots`/`x_redraw`). Typed characters, cursor movement and history recall
all render directly on the console.

The editor is active only while the shell is editing at the prompt. Before
and after (for example while a foreground child runs), the kernel tty is
back in cooked mode. So `^C` (`ISIG`) still kills the foreground child and
the shell survives (see the Ctrl+C docs and `trap.c`).

## Test

`keymap` (`user/tests/kernel.sh`, tag `console`) checks that init loaded
the Swiss map, that `keymap PATH` loads another, that a missing file is
refused and leaves the loaded map, and that a pty has no keymap.
