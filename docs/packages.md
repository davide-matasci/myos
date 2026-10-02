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
  `ROOT/var/lib/get-myos/index`; `-u` refreshes it), then each package's
  `<arch>-<name>.tar.gz`, checks its SHA-256 against the index and unpacks
  it under `ROOT` (default `/tmp/pkg`, on the tmpfs).
- Then **bind-mounts** the unpacked files where the image has them: a file
  under `bin/` by itself (`/tmp/pkg/bin/custom/vim` at `/bin/custom/vim`,
  since `/bin/custom` is a read-only tree with other programs in it),
  anything else at its second path component (`lib/vim`, `lib/os-test`,
  `lib/lynx.cfg`). Programs find their files at the usual paths, `PATH`
  needs no change, and `/proc/mounts` lists the binds. A bind lasts until
  reboot; `get-myos` records what it installed under
  `ROOT/var/lib/get-myos/pkgs/`.
- The mirror is `-m`, else `$MYOS_MIRROR`, else the project's rolling
  GitHub release (`.../releases/download/packages`). Downloads go through
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

A mirror is any HTTP server with these files in one directory. The flat
names are what GitHub release assets allow.

## In CI

- Boot-mini stays network-free. A **full boot** (`cargo run -- --ci`
  without `MYOS_CI_MINI`) packs this build's packages for its arch, serves
  `target/packages/` on the host's 127.0.0.1:8765 (the guest reaches it as
  `http://10.0.2.2:8765` on QEMU's user network) and, after the HTTPS
  stages, runs `get-myos -m http://10.0.2.2:8765 make` and the installed
  `make` (`CMD_GET_MYOS` in `src/wait_ci.rs`). The boot job needs `gzip`
  and `sha256sum`.
- Packages are published to the rolling GitHub release on pushes to
  master (see "Publishing" below once that lands).
