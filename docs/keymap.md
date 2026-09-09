# Loadable keyboard keymaps

myos does **not** ship a compiled-in US/CH ASCII table for live typing. The
PS/2 (x86) and virtio-input (aarch64/riscv64) drivers decode hardware events
to **stable keycodes** (PS/2 set-1 make codes / Linux `KEY_*` for the same
positions). Characters come from a keymap loaded at runtime.

## Boot behaviour

- Kernel keymap slot is **empty** after boot.
- Until a map is loaded, keyboard hardware produces **no** console ASCII.
- **Serial stdin is unchanged** (CI and serial consoles keep working).
- `user/init` loads the default map early (before getty):

```rust
const DEFAULT_KEYMAP: &[u8] = b"/lib/kbd/ch.map";
```

Switch the default to US by pointing that constant at `/lib/kbd/us.map`.

Both maps ship in the initramfs under `/lib/kbd/` (libfs nested tree — not
bootfs/`/etc`, which is flat and too small for reliable packing).

`user/init` loads CH first; on open/read/ioctl failure it prints a distinct
`[ FAIL ] keymap {open|read|ioctl} ch` line and falls back to `us.map` so the
PS/2 keyboard is never left without a map. Success prints `[ OK ] keymap ch`
or `[ OK ] keymap us`.

## ioctl API (`/dev/console`, also stdin/stdout tty fds)

| Request   | Value    | Argument |
|-----------|----------|----------|
| `KDSKMAP` | `0x5480` | Pointer to `{ len: u32, data: [u8; len] }` — map **text** (little-endian `len`, max 8 KiB). |
| `KDGKMAP` | `0x5481` | Pointer to `u32` out: `1` if a map is loaded, else `0`. |

Loaders must **loop `read`** until EOF: kernel `fd_read` caps each call at `FILE_IO_TMP` (2048), while maps may be larger (e.g. `ch.map` ≈ 2051).

`KDSKMAP` replaces any previously loaded map. On parse failure the previous
map is left unchanged, the syscall returns an error, and the kernel prints
`[ FAIL ] keymap: <reason>` on the console (serial/FB).

Numbers sit next to the existing termios ioctls (`TCGETS`/`TCSETS` =
`0x5401`/`0x5402`).

## Map file format (text)

Lines are:

```
keycode <N> = <base> <Shift> <AltGr> <Shift+AltGr>
```

- `<N>` is decimal or `0xNN` (keycode = PS/2 set-1 make).
- Each level token is one of:
  - a single printable byte (`a`, `/`, …)
  - `none` — empty (no character)
  - `ENTER` / `\n`, `TAB` / `\t`, `BKSP` / `\b`, `SPACE`
  - `\xNN` or `0xNN` — explicit byte (Latin-1 for CH letters)
- `#` starts a comment. Blank lines ignored.

### Example

```
keycode 0x18 = o O none none
keycode 0x08 = 7 / | none
keycode 0x35 = - _ none none
```

## Swiss German notes

`ch.map` is ISO Swiss German QWERTZ:

- Physical `/` is **Shift+7** (fixes the classic US-map `/` vs `_` mismatch).
- Physical `-`/`_` is the key US labels `/?`.
- **AltGr** (Right Alt) supplies `@ # | \ [ ] { }` for daily shell use.
- Accented letters are Latin-1 bytes (`\xFC` = ü, …). v1 is **not UTF-8**;
  the tty path is still a single-byte stream.

## Keycode sources

| Arch    | Hardware     | Keycode space                          |
|---------|--------------|----------------------------------------|
| x86_64  | PS/2 8042    | set-1 makes via `ps2-scancode`          |
| aarch64 | virtio-input | Linux `KEY_*` (same numbers for alphanumerics) |
| riscv64 | virtio-input | same as aarch64                        |

Modifiers tracked: **Shift**, **AltGr** (Right Alt), **Ctrl** (Left/Right).
Caps Lock is ignored in v1.

## Reserved keys (not keymap-driven)

A few keys are handled by the kernel before the loadable keymap, so they work
regardless of which map is loaded and are not bound in `us.map` / `ch.map`:

| Key          | Keycode | Output bytes            |
|--------------|---------|-------------------------|
| Esc          | `0x01`  | `0x1b`                  |
| Up arrow     | `0x48`  | `ESC [ A` (3 bytes)     |
| Down arrow   | `0x50`  | `ESC [ B`               |
| Right arrow  | `0x4D`  | `ESC [ C`               |
| Left arrow   | `0x4B`  | `ESC [ D`               |

**Ctrl + letter** produces the control byte (`ch & 0x1f`), so `Ctrl+C` → `0x03`
and hits the kernel's `ISIG` ^C path in both cooked and raw mode.

In **cooked** (canonical) mode the kernel swallows complete ESC/CSI sequences
(arrows, function keys) so `[` + letters never leak into a plain, canonical
command line; a lone Esc is dropped like bash. Cooked mode is what non-editing
readers see.

In **raw** mode (ICANON cleared) the real ESC bytes are delivered untouched —
this is how TUIs like `vim` and the shell's line editor get their keys.

## oksh: Emacs line editing on the raw console

The interactive shell (`ports/oksh`) does **not** read cooked canonical lines
anymore. At the `$` prompt it puts the console tty into raw/cbreak mode
(clears `ICANON`\|`ECHO`, keeps `ISIG` via `tcsetattr` → kernel `TCSETS`) and
runs its bundled **Emacs** line editor (`emacs.c`) over the raw bytes. The
kernel delivers the `ESC [ A/B/C/D` bytes the keyboard drivers produce directly
(no cooked swallowing in raw mode), and oksh maps them naturally:

| Key          | Bytes       | Emacs binding     | Effect        |
|--------------|-------------|-------------------|---------------|
| Up arrow     | `ESC [ A`   | `x_prev_com`      | history Up    |
| Down arrow   | `ESC [ B`   | `x_next_com`      | history Down  |
| Right arrow  | `ESC [ C`   | `x_mv_forw`       | cursor right  |
| Left arrow   | `ESC [ D`   | `x_mv_back`       | cursor left   |
| Home / End   | `ESC [ H/F` | `x_mv_begin/end`  | line start/end |

Because raw mode disables kernel echo, oksh's editor owns echo and redraw
(`x_zots`/`x_redraw`), so typed characters, cursor movement, and history recall
all render directly on the console. The editor is active only while the shell
is editing at the prompt; before/after (e.g. while a foreground child runs) the
kernel tty is restored to cooked mode, so `^C` (ISIG) still kills the
foreground child and the shell survives (see the Ctrl+C docs and `trap.c`).
