# Vim (FEAT_TINY) for myos

Upstream Vim pinned in `versions.env` (not vendored). Cross-built with newlib +
myos libgloss for `x86_64` / `aarch64` / `riscv64`.

## Layout

| Script | Role |
|--------|------|
| `fetch.sh` | Clone pinned tag into `target/vim-src` (idempotent) |
| `prepare.sh` | rsync → `target/vim-myos-build`, install `config.h` / stubs |
| `build.sh` | Cross-compile → `target/vim-<arch>-unknown-none` |

Thin wrappers: `scripts/fetch-vim.sh`, `scripts/build-vim.sh`.

## Config choice

`config.h` is **hand-written** for freestanding myos (not host `./configure`).
Host configure enables TERMINFO/`tgetent` (ncurses) and Linux-only APIs.
We keep `FEAT_TINY` + `UNIX`, disable terminfo, and use Vim’s **builtin
termcap** with builtin terms; set `TERM=dumb` at runtime if needed.

## Image path

Packed into initramfs as `/bin/custom/vim` (on `PATH` via `/bin/custom`).

## Limitations

- Kernel stdin is **cooked**; `tcgetattr` fails / `tcsetattr` is a no-op success
  (same spirit as oksh). Raw/visual mode and single-key input are limited.
- No ncurses — dumb/`builtin_terms` only; `TIOCGWINSZ` stub is 24×80.
- `select()` is a tiny stub (zero-timeout → idle; otherwise stdin “ready”).
- Interactive editing on serial/FB may be awkward; opening a file and `:q!`
  should work. Prefer `TERM=dumb`.

## Try after login

```sh
echo hello > /tmp/a.txt
vim /tmp/a.txt
# :q! to quit without saving
```
