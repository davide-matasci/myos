# Packages, apps and get-myos

A **package** is a port that CI builds and publishes but the image does not
carry. On a running system, `get-myos` fetches it from a mirror and installs
it as an **app**: a directory of its own per version,
`ROOT/<name>-<version>/`, with `ROOT/<name>` pointing at the current one,
that nothing outside it sees until `run-myos` runs it. Which ports are packages is
decided by their directory: `packages/<name>` instead of `ports/<name>`
(see `docs/ports.md`); nothing else changes, the descriptor (`port.env`) is
the same.

## On the guest

```sh
get-myos [-r ROOT] [-m MIRROR] [-u] [-l] PACKAGE...
get-myos [-r ROOT] -d PACKAGE... | --sync [FILE] | --gc
get-myos [-r ROOT] --generations | --rollback [PACKAGE...] | --switch N
get-myos [-r ROOT] --root
run-myos [-r ROOT] APP[@VERSION][,APP...][:CMD] [ARGS...]
```

- **The root** is `-r`, else `$MYOS_APPS`, else `/data/apps` when the boot
  disk's data partition is mounted at `/data` (`docs/install.md`), else
  `/tmp/apps` (the tmpfs, gone at reboot); `get-myos --root` prints it.
  An app in `/data/apps` is there at the next boot, ready to run.
- Downloads `<arch>-index.txt` (and `<arch>-packages.txt`) from the mirror
  once (into `ROOT/.get-myos/`; `-u` refreshes them), then streams
  each package's `<arch>-<name>.tar.gz` from `curl` through gunzip and tar
  straight into `ROOT/.<name>-<version>.new/`, checking the SHA-256 of the
  stream against the index at the end. The tarball is never stored: a
  tmpfs file holds at most 16 MiB and the riscv64 `os-test` package is
  bigger than that. Only when the checksum matches is the directory
  renamed to `ROOT/<name>-<version>/` and `ROOT/<name>` pointed at it
  (below); a failed download or a mismatch removes it and leaves the
  current version as it was. `get-myos` records the current version of
  each package under `ROOT/.get-myos/pkgs/`.
- **Dependencies**: a package's index line names the packages it needs on
  the running system (`PORT_RDEPS` in its descriptor, `docs/ports.md`:
  `st` needs `x11-xft` and `x11-fonts`, `x11-xft` needs `x11-libs`), each
  at the version it was built with (`x11-libs@9c34152496e6`), and
  `get-myos` installs them first, depth first, each once; `get-myos st`
  is the whole install. A package already installed at the index's
  version is skipped. The app's manifest names the same versions, and
  `run-myos` binds those (below): an app keeps the dependency it was built
  with even after that one is upgraded, as long as the version is on disk.
  A version the index does not have (an app from an older release whose
  dependency is gone) is refused.
- **Versions**: the index records each package's version (its build's
  input hash) and, in a header line, the build's **release**
  (`release=YYYYMMDDHHMM`, the commit date in UTC, so a later build has
  the greater number), commit and syscall **ABI** (`src/release.rs`). The
  image carries the same for the running system in `/lib/myos-release`;
  the x86_64 image build writes it to `target/myos-release`, and
  everything packed from `target/` afterwards (the other arches'
  initramfs, the packages) is stamped from that file, so a CI run that
  reuses cached images stamps its packages like them.
  `get-myos -u` refreshes the index and brings every installed package
  whose version changed to the new one (below). `get-myos -l` lists the
  mirror's packages: version, dependencies, `installed`, `upgrade`
  (installed at another version) or `-`, and the installed versions, the
  current one first and marked `*` (`lua 59d0a0afbd96 - installed
  59d0a0afbd96*,000000000000`), after the mirror's and the system's
  release and ABI.
- **Compatibility**: packages are rebuilt with every release (an ABI
  change is made outright, not worked around, `AGENTS.md`); `get-myos`
  refuses an index whose ABI is above the system's (its
  programs could call syscalls this kernel lacks) and says which release
  the system has. An index or an image from before the header has no ABI
  to compare, and installs as before.
- The mirror is `-m`, else `$MYOS_MIRROR`, else the one the root's index
  came from (`ROOT/.get-myos/mirror`, written with the index), else the
  project's rolling GitHub release (`.../releases/download/rolling`).
  Downloads go through `curl` (in every image, with the CA bundle). A
  failed index download leaves the one there before.

### Versions

Each version of an app has a directory of its own, `ROOT/<name>-<version>/`
(the version is the index's, the build's input hash), and `ROOT/<name>` is a
relative symlink to the current one (`ROOT/vim -> vim-f6df4b2886d8`). An
apps root therefore needs a filesystem with symlinks: the tmpfs or ext2
(`/data`), not FAT.

- **Upgrades are atomic**: the new version is unpacked beside the current
  one, and `ROOT/<name>` is flipped only after its checksum matched, by a
  new symlink renamed over the old one; `ROOT/<name>` names one complete
  version or the other at every moment. A program already running keeps
  the version it started with (`run-myos` binds the version's directory,
  not the pointer).
- **Old versions** stay on disk until `get-myos --gc` (below), so that
  every generation can be returned to. An app installed before versions
  (a plain directory `ROOT/<name>/`) is replaced by its first upgrade.
- **Rollback of one app**: `get-myos --rollback NAME` points `NAME` back at
  the version it had before (the latest earlier generation's where it
  differs, else its one other version on disk; a second rollback goes
  forward again) and records it. The next `get-myos -u` brings it to the
  index's version again, without a download when that version is still
  unpacked.
- **Side by side**: `run-myos NAME@VERSION` runs another installed version
  than the current one (`VERSION`: the start of its hash, as long as it
  names one), with what it needs at the versions its manifest names.

### The set of apps: apps.list, generations, removal

- **`ROOT/apps.list`** names the apps asked for, one per line (`#` starts
  a comment): `get-myos NAME` adds NAME, `get-myos -d NAME` takes it out;
  what they need is not listed, it comes with them. A root from before the
  list gets one at its first change, naming the installed apps that no
  other one needs.
- **`get-myos --sync`** makes the installed apps what the list says: it
  refreshes the index, installs what is missing, takes out every app that
  is neither listed nor needed by a listed one, and upgrades the rest.
  `--sync FILE` makes FILE the list first (a list copied from another
  machine). At boot, `/etc/rc` runs `get-myos -r /data/apps --sync` in the
  background when `/data/apps/apps.list` is there (`docs/install.md`), its
  output in `/tmp/get-myos-sync.log`; offline, it fails and changes
  nothing.
- **Removal**: `get-myos -d NAME` takes an app out (its pointer and its
  record) unless another installed app needs it, which it names. Its
  versions stay until `--gc`, and what it alone needed stays installed
  until a `--sync`.
- **Generations**: every command that changes the installed apps (an
  install, `-u`, `-d`, `--sync`, `--rollback NAME`) ends by writing a
  generation, `ROOT/.get-myos/generations/N`: each installed app, its
  version and whether it is `listed` or only `needed`;
  `generations/current` holds the current one's number. `get-myos
  --generations` lists them (`*` the current one, the apps only needed in
  parentheses):
  ```
  1   clear@29417d597877 lua@1e87d9743631
  2 * clear@29417d597877 lua@1e87d9743631 x11-apps@a0996143b823 (x11-libs@88095b575f5d)
  ```
  `get-myos --rollback` (no names) returns the whole set to the generation
  before the current one, and `--switch N` to generation N: every app's
  pointer, the apps the generation lacks taken out, and `apps.list` as it
  was then. The next change writes a generation after the last one. An
  `-u` that upgraded several apps at once is undone together.
- **`get-myos --gc`** drops every generation but the current one, then
  removes every version directory that generation does not hold (with the
  versions its apps' manifests need) and what an interrupted install left
  (`ROOT/.<name>-<version>.new`). No rollback reaches past it.

### Apps

An app's directory holds the port's files at the paths the image would
have them (`ROOT/vim/bin/custom/vim`, `ROOT/vim/lib/vim/vimrc`) and its
**manifest**, `app`, which the packager writes into the tarball
(`src/packages.rs`):

```
run /bin/custom/st
needs x11-xft@698a36aea37a x11-fonts@1399fe7cc6d8
```

`run` is the program `run-myos APP` starts (the one named after the port,
else its first in `bin/custom/`; a port without programs, a library such
as `x11-libs`, has none), `needs` the apps it uses (its `PORT_RDEPS`), at
the versions it was built with; `run-myos` binds those versions. A bare
name (an app installed before exact versions) is that app's current
version. When two apps of one view need two versions of a third, the
first one named wins.

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
- `run-myos lua@000000:/bin/sh` runs a version other than the current one
  (Versions, above).

The rights are the caller's: the namespace only adds what the apps hold,
under the `sys.pkg` label (`etc/policy`: `/tmp/apps/**` and
`/data/apps/**`). An app is not isolated from the rest of the system yet.

It shares its download, tar and gzip code with `get-alpine`
(`user/get-myos/pkgtools.c`; the Linux layer's installer bootstraps Alpine's
`apk` the same way from Alpine's repositories).

## The format

Built by `cargo run -- packages` into `target/packages/`, for the three
arches, from the same descriptor code the initramfs is packed with
(`src/packages.rs` and `install_port` in `src/initramfs.rs`): every port
with `PORT_FILES`, image ports included (that is how the mechanism is tested
while a port is in the image).

| File | Content |
|------|---------|
| `<arch>-<name>.tar.gz` | ustar, gzip `-n`: the port's files at their image paths (`bin/custom/vim`, `lib/vim/vimrc`), mode 0755/0644, mtime 0, and the app's manifest, `app` (above); a program's aliases (a multicall ELF's names, `bin/git`) are symlinks to the file that has its data, relative to their directory (the tmpfs has no hard links, and a copy each would multiply the program); reproducible for the same inputs |
| `<arch>-index.txt` | a header, `# myos release=<YYYYMMDDHHMM> commit=<short hash> abi=<syscall count>` (`src/release.rs`), then one line per package: `name version size sha256 file deps`; the version is the port's input hash (its stamp), a user program's the tarball's own hash; `deps` the runtime dependencies (`PORT_RDEPS`), comma separated, each at its version (`x11-libs@88095b575f5d`), `-` for none; a dependency is packed before what needs it. `cargo run -- packages` refuses a dependency that is not a package with files, or a loop |
| `<arch>-boot.txt` | the boot files of the release, for `get-myos --upgrade` and `--install` (`docs/install.md`): the index's header, then `kernel <size> <sha256> <arch>-kernel`, `initramfs <size> <sha256> <arch>-initramfs`, and `esp <path> <size> <sha256> <arch>-esp-<path>` for each of Limine's files on a boot disk's ESP (`EFI/BOOT/BOOTX64.EFI`, `boot/limine/limine.conf` for slot `a`, ...), which `--install` writes and `--upgrade` brings a disk's to. Every initramfs carries the same `esp` lines and files at `/lib/myos-boot/` (`--install --local`) |
| `<arch>-kernel`, `<arch>-initramfs` | the kernel as the boot disk has it (its loadable segments) and the initramfs of a default build (no Linux layer, whatever the build that wrote them has) |
| `<arch>-packages.txt` | the names of the ports the image does not carry (`packages/`): what there is to install; the index also has the image's ports, whose tarballs test the mechanism |

A mirror is any HTTP server with these files in one directory. The flat
names are what GitHub release assets allow.

The image has the same header's fields in `/lib/myos-release`
(`release=... commit=... abi=...`, from the checkout and
`kernel/src/user/syscall.rs` when the x86_64 image is built, kept in
`target/myos-release`): what `get-myos` compares the index with.

## In CI

- A test run that skips `net` stays network-free. One that does not
  (`cargo run -- test`, `docs/testing.md`) packs this build's packages
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
  runs and is still there after the reboot (`app_kept`, and `apps_synced`:
  the `--sync` `/etc/rc` ran for its list changed nothing), the versions
  of an app (`get_myos_versions`: lua at a made-up older version upgraded
  beside it by `-u`, either one run, a rollback, `-u` flipping forward
  without a download, `--gc` removing the old one) and the set of apps in
  a root of its own (`apps_set`: exact dependency versions, `apps.list`,
  `-d` refusing a needed app, `--rollback` and `--switch` of the whole
  set, `--sync` to a shorter list, `--gc`, the remembered mirror). The
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
test), tinyx (the X server, `Xfbdev`), x11-xft (FreeType, fontconfig and Xft for the X clients) and x11-fonts (DejaVu Sans Mono, and DejaVu Sans for what it lacks), dwm (the window manager), st (the terminal), dmenu (the menu), x11-apps (xev), bottom (`btm`, the system monitor), clear (ncurses' terminal-clearing program), get-alpine (the Linux layer's Alpine package installer, `docs/linux-compat.md`), nginx (the web server, `packages/nginx/README.md`), openssl (LibreSSL's command, `packages/openssl/README.md`), and ncurses and pcre2 (build dependencies: of vim, lynx and clear, of nginx; nothing in the
image). Everything a boot needs
stays in `ports/` (and zlib and LibreSSL, which get-myos and curl link). The full test list
installs them all; the ISO (`cargo run -- iso`) carries the image only.
