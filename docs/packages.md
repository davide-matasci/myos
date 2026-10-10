# Packages, apps and get-myos

A **package** is a port that CI builds and publishes but the image does not
carry. On a running system, `get-myos` fetches it from a mirror and installs
it as an **app**: a directory of its own, `ROOT/<name>/`, that nothing
outside it sees until `run-myos` runs it. Which ports are packages is
decided by their directory: `packages/<name>` instead of `ports/<name>`
(see `docs/ports.md`); nothing else changes, the descriptor (`port.env`) is
the same.

## On the guest

```sh
get-myos [-r ROOT] [-m MIRROR] [-u] [-l] PACKAGE...
get-myos [-r ROOT] --root
run-myos [-r ROOT] APP[,APP...][:CMD] [ARGS...]
```

- **The root** is `-r`, else `$MYOS_APPS`, else `/data/apps` when the boot
  disk's data partition is mounted at `/data` (`docs/install.md`), else
  `/tmp/apps` (the tmpfs, gone at reboot); `get-myos --root` prints it.
  An app in `/data/apps` is there at the next boot, ready to run.
- Downloads `<arch>-index.txt` (and `<arch>-packages.txt`) from the mirror
  once (into `ROOT/.get-myos/`; `-u` refreshes them), then streams
  each package's `<arch>-<name>.tar.gz` from `curl` through gunzip and tar
  straight into `ROOT/<name>/`, checking the SHA-256 of the stream against
  the index at the end (a mismatch leaves the package not recorded). The
  tarball is never stored: a tmpfs file holds at most 16 MiB and the
  riscv64 `os-test` package is bigger than that. `get-myos` records what it
  installed under `ROOT/.get-myos/pkgs/`.
- **Dependencies**: a package's index line names the packages it needs on
  the running system (`PORT_RDEPS` in its descriptor, `docs/ports.md`:
  `st` needs `x11-xft` and `x11-fonts`, `x11-xft` needs `x11-libs`), and
  `get-myos` installs them first, depth first, each once; `get-myos st`
  is the whole install. A package already installed at the index's
  version is skipped.
- **Versions**: the index records each package's version (its build's
  input hash) and, in a header line, the build's **release**
  (`release=YYYYMMDDHHMM`, the commit date in UTC, so a later build has
  the greater number), commit and syscall **ABI** (`src/release.rs`). The
  image carries the same for the running system in `/lib/myos-release`.
  `get-myos -u` refreshes the index and brings every installed package
  whose version changed to the new one (its files rewritten under
  `ROOT/<name>/`). `get-myos -l` lists the
  mirror's packages: version, dependencies, `installed`, `upgrade`
  (installed at another version) or `-`, after the mirror's and the
  system's release and ABI.
- **Compatibility**: packages are rebuilt with every release (an ABI
  change is made outright, not worked around, `AGENTS.md`); `get-myos`
  refuses an index whose ABI is above the system's (its
  programs could call syscalls this kernel lacks) and says which release
  the system has. An index or an image from before the header has no ABI
  to compare, and installs as before.
- The mirror is `-m`, else `$MYOS_MIRROR`, else the project's rolling
  GitHub release (`.../releases/download/rolling`). Downloads go through
  `curl` (in every image, with the CA bundle).

### Apps

An app's directory holds the port's files at the paths the image would
have them (`ROOT/vim/bin/custom/vim`, `ROOT/vim/lib/vim/vimrc`) and its
**manifest**, `app`, which the packager writes into the tarball
(`src/packages.rs`):

```
run /bin/custom/vim
needs x11-xft x11-fonts
```

`run` is the program `run-myos APP` starts (the one named after the port,
else its first in `bin/custom/`; a port without programs, a library such
as `x11-libs`, has none), `needs` the apps it uses (its `PORT_RDEPS`).

Nothing of an app is on `PATH` or bound anywhere: `run-myos` (a shell
script, `user/get-myos/run-myos`) runs it in a **namespace** of its own
(`sec ns`, `docs/security.md`): the whole system as the caller sees it,
plus the files of the app and, transitively, of what it needs, at the
paths they were built for. For each app it walks the tree and binds the
shallowest path the system lacks (`/lib/vim` to `ROOT/vim/lib/vim`), or a
file alone in a directory the system has (`/bin/custom/vim`, since
`/bin/custom` holds the image's programs). Nothing changes for any other
process, and nothing is left behind when the command exits. The manifest
and the app's tests (`lib/myos-tests`) are not part of the view. Within
the view, a directory lists the names bound below it too (`ls
/bin/custom` shows `vim`).

- `run-myos vim FILE` runs the app's `run`.
- `run-myos coreutils:ls` runs another program, found in the `bin/*/` of
  the view's apps (the named ones first); `run-myos os-test:/bin/sh
  SCRIPT` an absolute path, anything in the view.
- `run-myos tinyx,dwm:startx` gives several apps one view (an X session:
  tinyx's `startx` runs `Xfbdev` and dwm); `run-myos APP,...` without a
  command runs the first one's.

The rights are the caller's: the namespace only adds what the apps hold,
under the `sys.pkg` label (`etc/policy`: `/tmp/apps/**` and
`/data/apps/**`). An app is not isolated from the rest of the system yet.

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
| `<arch>-<name>.tar.gz` | ustar, gzip `-n`: the port's files at their image paths (`bin/custom/vim`, `lib/vim/vimrc`), mode 0755/0644, mtime 0, and the app's manifest, `app` (above); a program's aliases (a multicall ELF's names, `bin/git`) are symlinks to the file that has its data, relative to their directory (the tmpfs has no hard links, and a copy each would multiply the program); reproducible for the same inputs |
| `<arch>-index.txt` | a header, `# myos release=<YYYYMMDDHHMM> commit=<short hash> abi=<syscall count>` (`src/release.rs`), then one line per package: `name version size sha256 file deps`; the version is the port's input hash (its stamp), a user program's the tarball's own hash; `deps` the runtime dependencies (`PORT_RDEPS`), comma separated, `-` for none. `cargo run -- packages` refuses a dependency that is not a package with files, or a loop |
| `<arch>-boot.txt` | the boot files of the release, for `get-myos --upgrade` and `--install` (`docs/install.md`): the index's header, then `kernel <size> <sha256> <arch>-kernel`, `initramfs <size> <sha256> <arch>-initramfs`, and `esp <path> <size> <sha256> <arch>-esp-<path>` for each of Limine's files on a boot disk's ESP (`EFI/BOOT/BOOTX64.EFI`, `boot/limine/limine.conf` for slot `a`, ...), which `--install` writes and `--upgrade` brings a disk's to. Every initramfs carries the same `esp` lines and files at `/lib/myos-boot/` (`--install --local`) |
| `<arch>-kernel`, `<arch>-initramfs` | the kernel as the boot disk has it (its loadable segments) and the initramfs of a default build (no Linux layer, whatever the build that wrote them has) |
| `<arch>-packages.txt` | the names of the ports the image does not carry (`packages/`): what there is to install; the index also has the image's ports, whose tarballs test the mechanism |

A mirror is any HTTP server with these files in one directory. The flat
names are what GitHub release assets allow.

The image has the same header's fields in `/lib/myos-release`
(`release=... commit=... abi=...`, written by the initramfs packer from the
checkout and `kernel/src/user/syscall.rs`): what `get-myos` compares the
index with.

## In CI

- The quick test list (`test-mini`) stays network-free. The **full** one
  (`cargo run -- test-full`, `docs/testing.md`) packs this build's packages
  for its arch, serves `target/packages/` on the host's 127.0.0.1:8765 (the
  guest reaches it as `http://10.0.2.2:8765` on QEMU's user network;
  `index.txt` and `packages.txt` there are the arch's) and installs
  **every package of the build** as its first test (`install_packages` in
  `user/tests/run.sh`: first one package with dependencies alone, checking
  they came with it, then all of them) into `/tmp/apps` (`MYOS_APPS`: the
  test disk's 64 MiB data partition is too small for os-test); the tests
  that use one (git and coreutils in `heap`, os-test with `make`, the
  packages' own `test.sh`, which `run.sh` finds in the apps) run it with
  `run-myos`. get-myos's own test (`user/get-myos/test.sh`) then checks
  the listing, an app installed in the default root (`/data/apps`) that
  runs and is still there after the reboot (`app_kept`), and an upgrade
  (a package recorded at a stale version is fetched again by `-u`). The
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
test), tinyx (the X server, `Xfbdev`), x11-xft (FreeType, fontconfig and Xft for the X clients) and x11-fonts (DejaVu Sans Mono, and DejaVu Sans for what it lacks), dwm (the window manager), st (the terminal), dmenu (the menu), x11-apps (xev), bottom (`btm`, the system monitor), clear (ncurses' terminal-clearing program), get-alpine (the Linux layer's Alpine package fetcher, `docs/linux-compat.md`), and ncurses (a build dependency of vim, lynx and clear, nothing in the
image). Everything a boot needs
stays in `ports/` (and zlib, which get-myos links). The full test list
installs them all; the ISO (`cargo run -- iso`) carries the image only.
