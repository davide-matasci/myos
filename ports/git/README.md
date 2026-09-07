# Git (Phase 1 — local porcelain) for myos

Upstream Git pinned in `versions.env` (not vendored). Cross-built with newlib +
myos libgloss + static zlib for `x86_64` / `aarch64` / `riscv64`.

## Pins

| Component | Tag | Commit |
|-----------|-----|--------|
| git | `v2.55.0` | `e9019fcafe0040228b8631c30f97ae1adb61bcdc` |
| zlib | `v1.3.1` | `51b7f2abdade71cd9bb0e7a373ef2610ec6f9daf` (see `ports/zlib/versions.env`) |

## Layout

| Script | Role |
|--------|------|
| `fetch.sh` | Clone pinned tag into `target/git-src` (idempotent) |
| `prepare.sh` | rsync → `target/git-myos-build`, install `config.mak` / stubs |
| `build.sh` | Cross-compile → `target/git-<arch>-unknown-none` (links `ports/zlib`) |
| `myos-git-cc.sh` | Compile/link wrapper (crt0 + `libz.a` + newlib) |
| `config.mak` | Phase-1 `NO_*` flags + undo host-Linux uname detections |
| `include/` | Freestanding stubs (`syslog.h`, `sys/statvfs.h`, `sys/un.h`, …) |
| `myos_stubs.c` / `myos_compat.h` | Runtime + compile-only helpers |

Thin wrappers: `scripts/fetch-git.sh`, `scripts/build-git.sh` (and
`scripts/fetch-zlib.sh` / `scripts/build-zlib.sh`).

## Phase 1 limits (local only)

Built with (see `config.mak`):

- `NO_GETTEXT` `NO_ICONV` `NO_PERL` `NO_PYTHON` `NO_TCLTK` `NO_PTHREADS`
- `NO_OPENSSL` / `NO_CURL` (no HTTPS remotes yet — curl/mbedtls later)
- `NO_EXPAT` `NO_RUST` `NO_GITWEB` `NO_REGEX` (bundled compat regex)
- `NO_UNIX_SOCKETS` `NO_IPV6` `NO_NSEC` `NO_PREAD` `NO_GETPAGESIZE`

Intended local workflows: `init`, `add`, `commit`, `log`, `status`, `diff`,
branch/checkout basics on a writable filesystem. Remotes /
`clone`/`fetch`/`push` over HTTPS or SSH are **out of scope** for Phase 1.

Patches and compat live only under `ports/git/` — sources are fetched, never vendored.

## Image path

Packed into initramfs as `/bin/custom/git` and `/bin/git` (hardlink group; on
`PATH` via `/bin/custom`, like vim/oksh). Missing ELF is a **hard error** at
image pack time (CI always builds git).

`gitexecdir` / `prefix` bake to `/bin/custom` so the multicall binary never
looks for missing `/usr/libexec/git-core` helpers.

Guest `SHELL_PATH` baked as `/bin/custom/sh` (oksh).

CI `/heap` smoke: `git -C /tmp/gittest init` + config + add + commit + log/status
(`[ OK ] git` / `[ OK ] git commit`).

## Try after login

```sh
mkdir /tmp/repo && cd /tmp/repo
git init
echo hello > f.txt
git add f.txt
git -c user.email=a@b -c user.name=me commit -m 't'
git log
```

## Known gaps

- Guest exec of the full Phase-1 binary needs `MAX_EXPAND_PAGES` ≥ ~1080 (image
  span with BSS). Smaller caps made `execve` fail with ENOENT (`git: no such
  file or directory` from oksh) even though `/bin/custom/git` was packed.
- No network remotes (no curl/openssl in this port).
- `ftruncate` is a successful no-op in libgloss (no SYS_FTRUNCATE yet); enough for Phase-1 index write-after-fill.
- `getrandom` is a software LCG stand-in (not cryptographic).
- `utimensat` may be ROFS-stubbed depending on path; timestamps may not stick.
- No pthreads; FSMonitor / background helpers disabled.
- Full QEMU smoke of git porcelain is optional; cross-build + `cargo check` covered.
