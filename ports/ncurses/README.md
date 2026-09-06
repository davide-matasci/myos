# ncurses for myos

Upstream **ncurses 6.5** (fetched, not vendored). Cross-built as a static
`libncurses.a` (termcap + tinfo + curses base, no widechar / no terminfo DB;
fallbacks: `dumb`, `ansi`, `vt100`).

## Layout

| Script | Role |
|--------|------|
| `fetch.sh` | Download pinned tarball → `target/ncurses-src` (SHA-256 checked) |
| `prepare.sh` | Host `./configure` + header gen + myos cfg patch → `target/ncurses-myos-build` |
| `build.sh` | Cross-compile → `target/ncurses-<arch>/{lib,include}` |

## Why host-configure?

`config.sub` rejects `*-myos`, and `*-unknown-myos-cc` cannot link host
probe executables. Configure on the CI/build host, then override `CC` for
the archive only (`make … ../lib/libncurses.a`).

## Status

- Static lib builds for x86_64 / aarch64 / riscv64.
- Vim may link `-lncurses` when `HAVE_TGETENT` is enabled (see `ports/vim/`);
  FEAT_TINY still works with builtin termcap if ncurses is absent.
- No `tic`/terminfo database in the image yet — fallbacks only.

## Try

```sh
./ports/ncurses/build.sh
ls target/ncurses-x86_64/lib/libncurses.a
```
