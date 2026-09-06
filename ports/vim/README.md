# Vim (FEAT_TINY) for myos

Upstream Vim pinned in `versions.env` (not vendored). Cross-built with newlib +
myos libgloss + static ncurses termcap for `x86_64` / `aarch64` / `riscv64`.

## Layout

| Script | Role |
|--------|------|
| `fetch.sh` | Clone pinned tag into `target/vim-src` (idempotent) |
| `prepare.sh` | rsync → `target/vim-myos-build`, install `config.h` / stubs |
| `build.sh` | Cross-compile → `target/vim-<arch>-unknown-none` (links `ports/ncurses`) |

Thin wrappers: `scripts/fetch-vim.sh`, `scripts/build-vim.sh`.

## Config choice

`config.h` is **hand-written** for freestanding myos (not host `./configure`).
`HAVE_TGETENT` / `HAVE_TERMCAP_H` use `ports/ncurses` (static `libncurses.a`,
fallbacks `dumb`/`ansi`/`vt100`); no TERMINFO database in the image.

## termios

`<termios.h>` and `tcgetattr`/`tcsetattr` stubs live in
`toolchain/newlib/libgloss/myos/` (installed into the newlib sysroot) — not
under `ports/vim/`.

## Image path

Packed into initramfs as `/bin/custom/vim` (on `PATH` via `/bin/custom`).
Missing ELF is a **hard error** at image pack time (CI always builds vim).

## Limitations

- Kernel stdin is **cooked**; libgloss `<termios.h>` stubs (`tcgetattr` fails /
  `tcsetattr` no-op success). Raw/visual mode and single-key input are limited.
- ncurses without a terminfo DB — fallbacks only; prefer `TERM=dumb` or `ansi`.
- `select()` is a tiny stub (zero-timeout → idle; otherwise stdin “ready”).
- Interactive editing on serial/FB may be awkward; opening a file and `:q!`
  should work.

## Try after login

```sh
echo hello > /tmp/a.txt
vim /tmp/a.txt
# :q! to quit without saving
```
