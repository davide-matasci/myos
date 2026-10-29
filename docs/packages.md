# Packages, apps and get-myos

A **package** is a port that CI builds and publishes but the image does not
carry. On a running system, `get-myos` fetches it from a mirror and installs
it as an **app**. An app has a directory of its own per version,
`ROOT/<name>-<version>/`, with `ROOT/<name>` pointing at the current one.
Nothing outside the app sees it until `run-myos` runs it:

```sh
get-myos coreutils            # install uutils coreutils
run-myos coreutils:ls /tmp    # run one of its commands
```

A port is a package when its directory is `packages/<name>` instead of
`ports/<name>` (`docs/ports.md`). Nothing else changes: the descriptor
(`port.env`) is the same. Everything a boot needs stays in `ports/`, with
zlib and LibreSSL, which get-myos and curl link. The directories under
`packages/` are the list. Packages that only other packages build against
(ncurses, pcre2, sqlite, libedit) carry no files of their own.

## On the guest

```sh
get-myos [-r ROOT] [-m MIRROR] [-u] [-l] PACKAGE...
get-myos [-r ROOT] -d PACKAGE... | --sync [FILE] | --gc
get-myos [-r ROOT] --generations | --rollback [PACKAGE...] | --switch N
get-myos [-r ROOT] --global [PACKAGE...] | --unglobal PACKAGE...
get-myos [-r ROOT] --root
run-myos [-r ROOT] APP[@VERSION][,APP...][:CMD] [ARGS...]
```

The code is `user/get-myos` (`get-myos.c`, the shell script `run-myos`,
and `pkgtools.c`, the download, tar and gzip code it shares with
`get-alpine`).

### The root and the mirror

- **The root** is `-r`, else `$MYOS_APPS`, else `/data/apps` when the
  boot disk's data partition is mounted at `/data` (`docs/install.md`),
  else `/tmp/apps` (the tmpfs, gone at reboot). `get-myos --root` prints
  it. An app in `/data/apps` survives a reboot.
- **The mirror** is `-m`, else `$MYOS_MIRROR`, else the one the root's
  index came from (`ROOT/.get-myos/mirror`), else the project's rolling
  GitHub release (`.../releases/download/rolling`).

### Installing

`get-myos` keeps the mirror's index in `ROOT/.get-myos/`; `-u` refreshes
it. Each package's tarball is streamed from `curl` through gunzip and tar
into `ROOT/.<name>-<version>.new/`, never stored (in `/tmp` it would take
the kernel heap a second time). Only when the SHA-256 matches the index
does the directory become `ROOT/<name>-<version>/`. A failure leaves the
current version as it was. An install, `-u` or `--sync` downloads four
packages at once, each in a worker process.

**Dependencies.** A package's index line names the packages it needs on
the running system (`PORT_RDEPS`, `docs/ports.md`), each at the version it
was built with (`st` needs `x11-xft` and `x11-fonts`, and `x11-xft` needs
`x11-libs`). `get-myos` installs them first, so `get-myos st` is the whole
install. The app's manifest names the same versions and `run-myos` binds
those, so an app keeps the dependency it was built with after that one is
upgraded, as long as the version is on disk.

### Releases and compatibility

The index's header records the build's **release** (`YYYYMMDDHHMM`, the
commit date in UTC), its commit and its syscall **ABI**
(`src/release.rs`). The image carries the same in `/lib/myos-release`.

- `get-myos -l` lists the mirror's and the system's release and ABI, then
  the mirror's packages, each with its version, dependencies, state
  (`installed`, `upgrade` or `-`) and installed versions, the current one
  marked `*`: `lua 59d0a0afbd96 - installed 59d0a0afbd96*,000000000000`.
- Packages are rebuilt with every release. An ABI change is made
  outright, not worked around (`AGENTS.md`). So `get-myos` refuses an
  index whose ABI is above the system's, because its programs could call
  syscalls this kernel lacks.

### Versions

The version is the index's: the build's input hash. `ROOT/<name>` is a
relative symlink to the current one (`ROOT/vim -> vim-f6df4b2886d8`), so
an apps root needs a filesystem with symlinks: the tmpfs or ext2, not FAT.

- **Upgrades are atomic.** The new version is unpacked beside the current
  one, and `ROOT/<name>` is flipped by a symlink renamed over the old one.
  A running program keeps its version, because `run-myos` binds the
  version's directory, not the pointer.
- **Old versions** stay on disk until `get-myos --gc`, so that every
  generation can be returned to.
- **`get-myos --rollback NAME`** points one app back at the version it
  had before; a second rollback goes forward again. The next `-u` brings
  it to the index's version, without a download when that is unpacked.
- **`run-myos NAME@VERSION`** runs an installed version other than the
  current one. `VERSION` is the start of its hash.

### The set of apps

- **`ROOT/apps.list`** names the apps asked for, one per line (`#` starts
  a comment). `get-myos NAME` adds NAME, `get-myos -d NAME` takes it out.
  What they need is not listed; it comes with them.
- **`get-myos --sync`** makes the installed apps what the list says:
  installs what is missing, takes out what is neither listed nor needed,
  upgrades the rest. `--sync FILE` first makes FILE the list. At boot,
  `/etc/rc` runs `get-myos -r /data/apps --sync` in the background when
  `/data/apps/apps.list` exists (`docs/install.md`), logging to
  `/tmp/get-myos-sync.log`.
- **`get-myos -d NAME`** refuses an app another installed app needs, and
  names it.
- **Generations**: every command that changes the installed apps writes
  one, `ROOT/.get-myos/generations/N`. `get-myos --generations` lists
  them:
  ```
  1   clear@29417d597877 lua@1e87d9743631
  2 * clear@29417d597877 lua@1e87d9743631 x11-apps@a0996143b823 (x11-libs@88095b575f5d)
  ```
  `get-myos --rollback` with no names returns the whole set (pointers and
  `apps.list`) to the generation before the current one, and `--switch N`
  to generation N.
- **`get-myos --gc`** drops every generation but the current one, the
  versions it does not hold and what an interrupted install left. No
  rollback reaches past it.

### Apps and run-myos

An app's directory holds the port's files at the paths the image would
have them (`ROOT/vim/bin/custom/vim`, `ROOT/vim/lib/vim/vimrc`), and its
**manifest**, `app`:

```
run /bin/custom/st
needs x11-xft@698a36aea37a x11-fonts@1399fe7cc6d8
```

`run` is the program `run-myos APP` starts (a library such as `x11-libs`
has none); `needs` names the apps it uses, at their versions.

Nothing of an app is on `PATH`. `run-myos` runs it in a **namespace** of
its own (`sec ns`, `docs/security.md`): the whole system as the caller
sees it, plus the files of the app and of what it needs, at the paths
they were built for. It binds the shallowest path the system lacks
(`/lib/vim`), or a file alone where the system has the directory
(`/bin/custom/vim`). Nothing changes for any other process.

- `run-myos vim FILE` runs the app's `run`.
- `run-myos coreutils:ls` runs another program of the view's apps;
  `run-myos os-test:/bin/sh SCRIPT` runs an absolute path.
- `run-myos tinyx,dwm:startx` gives several apps one view (an X session).
- `run-myos lua@000000:/bin/sh` runs a version other than the current one.

The rights are the caller's. The namespace only adds what the apps hold,
under the `sys.pkg` label (`etc/policy`). An app is not isolated from the
rest of the system yet.

### Global apps

`get-myos --global NAME` binds an app's files, and those of what it needs,
into the system tree for every process (`mount SOURCE TARGET bind`, by
`run-myos`'s rule), so its programs are on `PATH` with no `run-myos`.
`get-myos --unglobal NAME` takes them away. Binds change the tree for
everyone, so they need `write` on `kernel.mounts`, which is root's
(`etc/policy`).

- The global set is `ROOT/.get-myos/global/`, the binds made are in
  `ROOT/.get-myos/binds`. Each root has its own set.
- Every command that moves versions binds the global apps again at the
  versions now current. Generations do not record the global set.
- A path two apps would provide is refused, naming the app that has it.
- `get-myos --global` without a name binds the set again; `/etc/rc` runs
  it for `/data/apps` at boot (`docs/install.md`).

## The format

`cargo run -- packages` builds the packages into `target/packages/` for
the three arches (`src/packages.rs`, sharing the initramfs packer's
descriptor code). It packs every port with `PORT_FILES`, image ports
included, so the mechanism is tested while a port is in the image.

| File | Content |
|------|---------|
| `<arch>-<name>.tar.gz` | reproducible ustar: the port's files at their image paths and the manifest `app`. A multicall program's aliases are relative symlinks, since the tmpfs has no hard links |
| `<arch>-index.txt` | a header, `# myos release=<YYYYMMDDHHMM> commit=<short hash> abi=<syscall count>`, then `name version size sha256 file deps` per package; `deps` is comma separated (`x11-libs@88095b575f5d`) or `-` |
| `<arch>-boot.txt` | the release's boot files, for `get-myos --upgrade` and `--install` (`docs/install.md`) |
| `<arch>-kernel`, `<arch>-initramfs` | the kernel and the initramfs of a default build (no Linux layer) |
| `<arch>-packages.txt` | the names of the ports the image does not carry: what there is to install |

A mirror is any HTTP server with these files in one directory. The names
are flat because GitHub release assets allow only that.

The image's `/lib/myos-release` has the header's fields, written to
`target/myos-release` when the x86_64 image is built. Everything packed
from `target/` afterwards is stamped from that file, so a CI run that
reuses cached images stamps its packages like them.

## In the boot tests

A test run that does not skip `net` (`docs/testing.md`) packs this
build's packages for its arch and serves `target/packages/` on the host's
127.0.0.1:8765, the guest's `http://10.0.2.2:8765`. When a `pkg` test is
selected, the runner's first test installs the packages the selected
tests name in their tags into `/tmp/apps` (`install_packages` in
`user/tests/run.sh`; the test disk's data partition is too small for
os-test). The tests run them with `run-myos`. get-myos's own tests are in
`user/get-myos/test.sh`.

## Publishing

On master, once the build, the boots and the ISO passed, the `publish`
job of `ci-runtime.yml` uploads `target/packages/*` and
`myos-x86_64.iso` to the rolling prerelease **`rolling`** and moves its
tag to the published commit. It is get-myos's default mirror
(`https://github.com/davide-matasci/myos/releases/download/rolling`).

The copyleft ports (git, lynx, GNU make, ...) are redistributed there as
binaries. The tag names the commit they were built from, whose
`versions.env` pins and `*.myos.patch` files are the corresponding source
(`THIRD_PARTY_NOTICES.md`, "License compliance" in `AGENTS.md`).
