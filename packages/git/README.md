# Git for myos: the porcelain and SSH remotes

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
| `config.mak` | `NO_*` flags + undo host-Linux uname detections |
| `include/` | Headers newlib lacks (`sys/statvfs.h`, `netinet/tcp.h`) |
| `myos_stubs.c` / `myos_compat.h` | What libgloss has no honest implementation for (`prctl`, `sync_file_range`, `statvfs`) + compile-only definitions |

Thin wrappers: `scripts/fetch-git.sh`, `scripts/build-git.sh` (and
`scripts/fetch-zlib.sh` / `scripts/build-zlib.sh`).

## Limits

Built with (see `config.mak`):

- `NO_GETTEXT` `NO_ICONV` `NO_PERL` `NO_PYTHON` `NO_TCLTK` `NO_PTHREADS`
- `NO_OPENSSL` `NO_CURL` `NO_EXPAT` (no HTTP(S) remotes: below)
- `NO_RUST` `NO_GITWEB` `NO_REGEX` (bundled compat regex)
- `NO_UNIX_SOCKETS` `NO_IPV6` `NO_NSEC` `NO_PREAD` `NO_GETPAGESIZE`

## Remotes

- **HTTP(S)**: not yet. Git is GPL-2.0-only, and libcurl's TLS here is
  LibreSSL, whose libcrypto keeps the OpenSSL license: the two in one
  binary are the classic GPLv2 incompatibility. HTTPS waits on a
  compatible TLS library (issue #591).
- **SSH**: git runs `ssh`, which is dropbear's client (`dbclient`, also
  `/bin/custom/ssh`): `git clone ssh://user@host[:port]/path` or
  `user@host:path`. A key is `ssh -i KEY` (`GIT_SSH_COMMAND` or
  `core.sshCommand`; `dropbearkey -t ed25519 -f KEY` makes one, `-y` prints
  its public half) and an unknown host key is asked about on the terminal
  (`ssh -y` accepts it). dbclient warns about git's `-o SendEnv=GIT_PROTOCOL`
  and goes on.
- **Serving** over SSH from myos (its dropbear, `docs/ssh.md`): the client's
  command runs in the login's shell, where git is an app, not on `PATH`:
  `git clone -u 'run-myos git upload-pack' ...` (and `--receive-pack` for a
  push), or `get-myos --global git`, which puts `git-upload-pack` and
  `git-receive-pack` (aliases of the one binary) in `/bin/custom`.

The test (`test.sh`): a clone over SSH from the system itself (its own
dropbear, a key made by the test).

## Image path

A package (`get-myos git`): the app holds `/bin/custom/git` and `/bin/git`,
with `git-upload-pack`, `git-receive-pack` and `git-upload-archive` as
symlinks to it.

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

- Guest exec of the full binary needs `MAX_EXPAND_PAGES` ≥ ~1080 (image
  span with BSS). Smaller caps made `execve` fail with ENOENT (`git: no such
  file or directory` from oksh) even though `/bin/custom/git` was packed.
- `ftruncate` is a successful no-op in libgloss (no SYS_FTRUNCATE yet); enough for the index write-after-fill.
- `getrandom` is a software LCG stand-in (not cryptographic).
- `chmod` / `fchmodat` are successful no-ops (the VFS keeps no modes); `utime` / `utimensat` set real access and modification times (libgloss).
- Built with `NO_MMAP` (file-backed mmap is not implemented; git uses read+malloc).
- `_fstat` must report real `st_size` via the open-path table (a zero size made config rewrite crash).
- Newlib must be built with `HAVE_RENAME` so `rename()` uses libgloss `_rename` (not `link`+`unlink`; `_link` is still EROFS).
- tmpfs `rename` must replace an existing non-directory dest (POSIX); git config updates are lock+rename onto `.git/config`.
- No pthreads; FSMonitor / background helpers disabled.
- Full QEMU smoke of git porcelain is optional; cross-build + `cargo check` covered.
