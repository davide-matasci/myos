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
const DEFAULT_KEYMAP: &[u8] = b"/etc/kbd/ch.map";
```

Switch the default to US by pointing that constant at `/etc/kbd/us.map`.

Both maps ship in the initramfs under `/etc/kbd/`.

## ioctl API (`/dev/console`, also stdin/stdout tty fds)

| Request   | Value    | Argument |
|-----------|----------|----------|
| `KDSKMAP` | `0x5480` | Pointer to `{ len: u32, data: [u8; len] }` — map **text** (little-endian `len`, max 8 KiB). |
| `KDGKMAP` | `0x5481` | Pointer to `u32` out: `1` if a map is loaded, else `0`. |

`KDSKMAP` replaces any previously loaded map. On parse failure the previous
map is left unchanged and the syscall returns an error.

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

Modifiers tracked: **Shift**, **AltGr** (Right Alt). Caps Lock is ignored in v1.
