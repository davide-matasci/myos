# Packages and get-myos

A **package** is a port that CI builds and publishes but the image does not
carry. On a running system, `get-myos` fetches it from a mirror and makes
its files appear where the image would have had them. Which ports are
packages is decided by their directory: `packages/<name>` instead of
`ports/<name>` (see `docs/ports.md`); nothing else changes, the descriptor
(`port.env`) is the same.

## On the guest

```sh
get-myos [-r ROOT] [-m MIRROR] [-u] PACKAGE...
```

- Downloads `<arch>-index.txt` from the mirror once (into
  `ROOT/var/lib/get-myos/index`; `-u` refreshes it), then streams each
  package's `<arch>-<name>.tar.gz` from `curl` through gunzip and tar
  straight into `ROOT` (default `/tmp/pkg`, on the tmpfs), checking the
  SHA-256 of the stream against the index at the end (a mismatch leaves the
  files unbound and the package not recorded). The tarball is never stored:
  a tmpfs file holds at most 16 MiB and the riscv64 `os-test` package is
  bigger than that.
- Then **bind-mounts** the unpacked files where the image has them: the
  first directory of a file's path that the running system lacks
  (`/tmp/pkg/lib/vim` at `/lib/vim`, `lib/os-test`), or the file itself
  when every directory above it exists (`/tmp/pkg/bin/custom/vim` at
  `/bin/custom/vim`, since `/bin/custom` is a read-only tree with other
  programs in it; a package's test script into `/lib/myos-tests/ports/`).
  Programs find their files at the usual paths, `PATH` needs no change,
  and `/proc/mounts` lists the binds. A bind lasts until reboot; `get-myos`
  records what it installed under `ROOT/var/lib/get-myos/pkgs/`.
- The mirror is `-m`, else `$MYOS_MIRROR`, else the project's rolling
  GitHub release (`.../releases/download/rolling`). Downloads go through
  `curl` (in every image, with the CA bundle).
- `ROOT` can be anywhere writable (`-r /ext2/pkg` on the ext2 disk); only
  the binds are lost at reboot, running `get-myos` again re-binds without
  downloading.

It shares its download, tar and gzip code with `get-alpine`
(`user/get-myos/pkgtools.c`; the Linux layer's installer is the same
pattern for Alpine's repositories).

## The format

Built by `cargo run -- packages` into `target/packages/`, for the three
arches, from the same descriptor code the initramfs is packed with
(`src/packages.rs` and `install_port` in `src/initramfs.rs`): every port
with `PORT_FILES`, image ports included (that is how the mechanism is tested
while a port is in the image).

| File | Content |
|------|---------|
| `<arch>-<name>.tar.gz` | ustar, gzip `-n`: the port's files at their image paths (`bin/custom/vim`, `lib/vim/vimrc`), mode 0755/0644, mtime 0, a program's aliases as files of their own; reproducible for the same inputs |
| `<arch>-index.txt` | one line per package: `name version size sha256 file`; the version is the port's input hash (its stamp), a user program's the tarball's own hash |
| `<arch>-packages.txt` | the names of the ports the image does not carry (`packages/`): what there is to install; the index also has the image's ports, whose tarballs test the mechanism |

A mirror is any HTTP server with these files in one directory. The flat
names are what GitHub release assets allow.

## In CI

- The quick test list (`test-mini`) stays network-free. The **full** one
  (`cargo run -- test-full`, `docs/testing.md`) packs this build's packages
  for its arch, serves `target/packages/` on the host's 127.0.0.1:8765 (the
  guest reaches it as `http://10.0.2.2:8765` on QEMU's user network;
  `index.txt` and `packages.txt` there are the arch's) and installs
  **every package of the build** as its first test (`install_packages` in
  `user/tests/run.sh`), so the tests that use one (git in `heap`, os-test
  with `make`, the packages' own `test.sh`) find it at its image path. The
  boot job needs `gzip` and `sha256sum`.
- The build job writes the packages of the three arches (`cargo run --
  packages`) and uploads them as the `myos-packages` artifact (7 days).

## Publishing

On master (a push, or the daily scheduled full boot), once the build, the
boots and the ISO passed, the `publish` job of `ci-runtime.yml` uploads
`target/packages/*` and `myos-x86_64.iso` to the rolling GitHub release
**`rolling`** (a prerelease; `--clobber` replaces the files) and moves its
tag to the published commit. That release is get-myos's default mirror
(`https://github.com/davide-matasci/myos/releases/download/rolling`).

The copyleft ports (git, lynx, GNU make, ...) are redistributed as
binaries there: the tag names the commit they were built from, whose
`versions.env` pins and `*.myos.patch` files are the corresponding source
(`THIRD_PARTY_NOTICES.md`, "License compliance" in `AGENTS.md`).

## What is a package today

`packages/`: git, lua, lynx, make, os-test, vim, x11-libs (the X client
libraries for the X packages, with libX11's error database and their
test), tinyx (the X server, `Xfbdev`), x11-xft (FreeType, fontconfig and Xft for the X clients) and x11-fonts (DejaVu Sans Mono), dwm (the window manager), st (the terminal), dmenu (the menu), and ncurses (a build dependency of vim and lynx, nothing in the
image). Everything a boot needs
stays in `ports/` (and zlib, which get-myos links). The full test list
installs them all; the ISO (`cargo run -- iso`) carries the image only.
