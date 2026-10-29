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
(`port.env`) is the same.

## On the guest

```sh
get-myos [-r ROOT] [-m MIRROR] [-u] [-l] PACKAGE...
get-myos [-r ROOT] -d PACKAGE... | --sync [FILE] | --gc
get-myos [-r ROOT] --generations | --rollback [PACKAGE...] | --switch N
get-myos [-r ROOT] --global [PACKAGE...] | --unglobal PACKAGE...
get-myos [-r ROOT] --root
run-myos [-r ROOT] APP[@VERSION][,APP...][:CMD] [ARGS...]
```

### The root and the mirror

- **The root** is `-r`, else `$MYOS_APPS`, else `/data/apps` when the
  boot disk's data partition is mounted at `/data` (`docs/install.md`),
  else `/tmp/apps` (the tmpfs, gone at reboot). `get-myos --root` prints
  it. An app in `/data/apps` is still there at the next boot, ready to
  run.
- **The mirror** is `-m`, else `$MYOS_MIRROR`, else the one the root's
  index came from (`ROOT/.get-myos/mirror`, written with the index), else
  the project's rolling GitHub release (`.../releases/download/rolling`).
- Downloads go through `curl`, which every image has, with the CA bundle.

### Installing

`get-myos` downloads `<arch>-index.txt` and `<arch>-packages.txt` from the
mirror once, into `ROOT/.get-myos/`. `-u` refreshes them. A failed index
download leaves the previous index in place.

For each package, `get-myos` streams `<arch>-<name>.tar.gz` from `curl`
through gunzip and tar straight into `ROOT/.<name>-<version>.new/`. At the
end it checks the SHA-256 of the stream against the index.

- The tarball is never stored. In `/tmp` it would take the kernel heap a
  second time, beside the files it unpacks (the riscv64 `os-test` package
  is 49 MB).
- Only when the checksum matches does `get-myos` rename the directory to
  `ROOT/<name>-<version>/` and point `ROOT/<name>` at it ("Versions",
  below).
- A failed download or a mismatch removes the directory and leaves the
  current version as it was.
- `get-myos` records the current version of each package under
  `ROOT/.get-myos/pkgs/`.

**Four at a time.** An install, an upgrade (`-u`) or a `--sync` downloads
the packages named and what they need whose version is not on disk yet.
It streams four packages at once, each in a worker process of its own,
with its own `curl`, into its own `.new` directory.

- The recording (`ROOT/<name>`, the records, `apps.list`, the generation)
  stays with the one main process. It runs in order, once the workers are
  done.
- A package whose worker failed fails the install, as it would if
  downloaded alone. The worker prints the message, each message one whole
  line.
- A single package downloads in the main process itself.

**Dependencies.** A package's index line names the packages it needs on
the running system (`PORT_RDEPS` in its descriptor, `docs/ports.md`), each
at the version it was built with (`x11-libs@9c34152496e6`). For example,
`st` needs `x11-xft` and `x11-fonts`, and `x11-xft` needs `x11-libs`.

- `get-myos` installs the dependencies first, depth first, each once, so
  `get-myos st` is the whole install.
- A package already installed at the index's version is skipped.
- The app's manifest names the same versions, and `run-myos` binds those
  (below). So an app keeps the dependency it was built with even after
  that one is upgraded, as long as the version is on disk.
- A version the index does not have is refused (an app from an older
  release whose dependency is gone).

### Releases and compatibility

The index records each package's version: its build's input hash. A
header line records the build's **release** (`release=YYYYMMDDHHMM`, the
commit date in UTC, so a later build has the greater number), its commit
and its syscall **ABI** (`src/release.rs`).

The image carries the same for the running system in `/lib/myos-release`.
The x86_64 image build writes it to `target/myos-release`. Everything
packed from `target/` afterwards (the other arches' initramfs, the
packages) is stamped from that file, so a CI run that reuses cached images
stamps its packages like them.

- `get-myos -u` refreshes the index and brings every installed package
  whose version changed to the new one ("Versions", below).
- `get-myos -l` lists the mirror's and the system's release and ABI, then
  the mirror's packages. Each line has the version, the dependencies,
  `installed`, `upgrade` (installed at another version) or `-`, and the
  installed versions, the current one first and marked `*`:
  `lua 59d0a0afbd96 - installed 59d0a0afbd96*,000000000000`.
- Packages are rebuilt with every release. An ABI change is made
  outright, not worked around (`AGENTS.md`).
- `get-myos` refuses an index whose ABI is above the system's, because
  its programs could call syscalls this kernel lacks. It says which
  release the system has.
- An index or an image from before the header has no ABI to compare, and
  installs without the check.

### Versions

Each version of an app has a directory of its own,
`ROOT/<name>-<version>/`. The version is the index's: the build's input
hash. `ROOT/<name>` is a relative symlink to the current one
(`ROOT/vim -> vim-f6df4b2886d8`). An apps root therefore needs a
filesystem with symlinks: the tmpfs or ext2 (`/data`), not FAT.

- **Upgrades are atomic.** The new version is unpacked beside the current
  one. `ROOT/<name>` is flipped only after the checksum matched, by a new
  symlink renamed over the old one. So `ROOT/<name>` names one complete
  version or the other at every moment. A program already running keeps
  the version it started with, because `run-myos` binds the version's
  directory, not the pointer.
- **Old versions** stay on disk until `get-myos --gc` (below), so that
  every generation can be returned to. An app installed before versions
  existed (a plain directory `ROOT/<name>/`) is replaced by its first
  upgrade.
- **Rollback of one app**: `get-myos --rollback NAME` points `NAME` back
  at the version it had before, and records it. That is the latest
  earlier generation's version where it differs, else its one other
  version on disk. A second rollback goes forward again. The next
  `get-myos -u` brings it to the index's version again, without a
  download when that version is still unpacked.
- **Side by side**: `run-myos NAME@VERSION` runs an installed version
  other than the current one, with what it needs at the versions its
  manifest names. `VERSION` is the start of its hash, as long as it names
  one version.

### The set of apps: apps.list, generations, removal

- **`ROOT/apps.list`** names the apps asked for, one per line (`#` starts
  a comment). `get-myos NAME` adds NAME, and `get-myos -d NAME` takes it
  out. What they need is not listed; it comes with them. A root from
  before the list gets one at its first change, naming the installed apps
  that no other one needs.
- **`get-myos --sync`** makes the installed apps what the list says. It
  refreshes the index, installs what is missing, takes out every app that
  is neither listed nor needed by a listed one, and upgrades the rest.
  `--sync FILE` first makes FILE the list (a list copied from another
  machine). At boot, `/etc/rc` runs `get-myos -r /data/apps --sync` in
  the background when `/data/apps/apps.list` exists (`docs/install.md`),
  with its output in `/tmp/get-myos-sync.log`. Offline, it fails and
  changes nothing.
- **Removal**: `get-myos -d NAME` takes an app out (its pointer and its
  record), unless another installed app needs it; it then names that app.
  The removed app's versions stay until `--gc`, and what it alone needed
  stays installed until a `--sync`.
- **Generations**: every command that changes the installed apps (an
  install, `-u`, `-d`, `--sync`, `--rollback NAME`) ends by writing a
  generation, `ROOT/.get-myos/generations/N`. It lists each installed
  app, its version and whether it is `listed` or only `needed`.
  `generations/current` holds the current one's number. `get-myos
  --generations` lists them, with `*` on the current one and the apps
  that are only needed in parentheses:
  ```
  1   clear@29417d597877 lua@1e87d9743631
  2 * clear@29417d597877 lua@1e87d9743631 x11-apps@a0996143b823 (x11-libs@88095b575f5d)
  ```
  `get-myos --rollback` with no names returns the whole set to the
  generation before the current one, and `--switch N` to generation N.
  That restores every app's pointer, takes out the apps the generation
  lacks, and restores `apps.list` as it was then. The next change writes
  a generation after the last one. An `-u` that upgraded several apps at
  once is undone together.
- **`get-myos --gc`** drops every generation but the current one. It then
  removes every version directory that generation does not hold (keeping
  the versions its apps' manifests need), and what an interrupted install
  left (`ROOT/.<name>-<version>.new`). No rollback reaches past it.

### Apps

An app's directory holds the port's files at the paths the image would
have them (`ROOT/vim/bin/custom/vim`, `ROOT/vim/lib/vim/vimrc`), and its
**manifest**, `app`, which the packager writes into the tarball
(`src/packages.rs`):

```
run /bin/custom/st
needs x11-xft@698a36aea37a x11-fonts@1399fe7cc6d8
```

- `run` is the program `run-myos APP` starts. It is the one named after
  the port, in `bin/custom/` or another directory of `bin/` (tcc's
  `/bin/tcc/tcc`), else the port's first program in `bin/custom/`. A port
  without programs, a library such as `x11-libs`, has no `run`.
- `needs` names the apps it uses (its `PORT_RDEPS`), at the versions it
  was built with. `run-myos` binds those versions. A bare name (an app
  installed before exact versions) means that app's current version.
- When two apps of one view need two versions of a third, the first one
  named wins.

Nothing of an app is on `PATH` or bound anywhere. `run-myos` (a shell
script, `user/get-myos/run-myos`) runs it in a **namespace** of its own
(`sec ns`, `docs/security.md`). The namespace holds the whole system as
the caller sees it, plus the files of the app and, transitively, of what
it needs, at the paths they were built for.

- For each app, `run-myos` walks the tree and binds the shallowest path
  the system lacks (`/lib/vim` to `ROOT/vim/lib/vim`). Where the system
  has the directory, it binds a file alone (`/bin/custom/vim`, since
  `/bin/custom` holds the image's programs).
- Nothing changes for any other process, and nothing is left behind when
  the command exits.
- The manifest and the app's tests (`lib/myos-tests`) are not part of the
  view.
- Within the view, a directory lists the names bound below it too (`ls
  /bin/custom` shows `vim`).

Examples:

- `run-myos vim FILE` runs the app's `run`.
- `run-myos coreutils:ls` runs another program, found in the `bin/*/` of
  the view's apps, the named ones first. `run-myos os-test:/bin/sh
  SCRIPT` runs an absolute path: anything in the view.
- `run-myos tinyx,dwm:startx` gives several apps one view (an X session:
  tinyx's `startx` runs `Xfbdev` and dwm). `run-myos APP,...` without a
  command runs the first app's `run`.
- `run-myos lua@000000:/bin/sh` runs a version other than the current one
  ("Versions", above).

The rights are the caller's. The namespace only adds what the apps hold,
under the `sys.pkg` label (`etc/policy`: `/tmp/apps/**` and
`/data/apps/**`). An app is not isolated from the rest of the system yet.

### Global apps

`get-myos --global NAME` makes an app part of the system for every
process. Its files, and those of what it needs, are bound where the image
would have them (`mount SOURCE TARGET bind`), by `run-myos`'s rule: the
shallowest path the system lacks, or a file alone in a directory it has.
So its programs are on `PATH` (`/bin/custom/vim`) with no `run-myos`.
`get-myos --unglobal NAME` takes them away (`umount TARGET` removes a
bind), together with what only that app needed. Binds change the tree for
everyone, so they need `write` on `kernel.mounts`, which is root's
(`etc/policy`).

- The global apps are `ROOT/.get-myos/global/<name>`. The binds get-myos
  made are in `ROOT/.get-myos/binds` (`target source app`, those
  `/proc/mounts` still shows).
- A bind names a version's directory (`ROOT/vim-<version>/`). So every
  command that moves versions (an install, `-u`, `--sync`, `--rollback`,
  `--switch`) binds the global apps again at the versions now current,
  and what they need at the versions their manifests name.
- Taking an app out (`-d`, `--sync`, a generation without it) takes it
  out of the global set first. Generations do not record the global set.
- A path two apps would provide (two global apps, or one global app's
  dependency at two versions) is refused, naming the app that has it.
  Nothing is bound then.
- `get-myos --global` without a name binds the global apps again.
  `/etc/rc` runs it for `/data/apps` at boot, before the first login
  (`docs/install.md`).
- Each root has its own global set. Two roots' global apps are not
  checked against each other.

`get-myos` shares its download, tar and gzip code with `get-alpine`
(`user/get-myos/pkgtools.c`). The Linux layer's installer bootstraps
Alpine's `apk` the same way, from Alpine's repositories.

## The format

`cargo run -- packages` builds the packages into `target/packages/`, for
the three arches. It uses the same descriptor code the initramfs is
packed with (`src/packages.rs` and `install_port` in `src/initramfs.rs`).
It packs every port with `PORT_FILES`, image ports included: that is how
the mechanism is tested while a port is in the image.

| File | Content |
|------|---------|
| `<arch>-<name>.tar.gz` | ustar, gzip `-n`: the port's files at their image paths (`bin/custom/vim`, `lib/vim/vimrc`), mode 0755/0644, mtime 0, and the app's manifest, `app` (above). A program's aliases (a multicall ELF's names, `bin/git`) are symlinks to the file that has its data, relative to their directory: the tmpfs has no hard links, and a copy each would multiply the program. Reproducible for the same inputs. |
| `<arch>-index.txt` | a header, `# myos release=<YYYYMMDDHHMM> commit=<short hash> abi=<syscall count>` (`src/release.rs`), then one line per package: `name version size sha256 file deps`. The version is the port's input hash (its stamp); a user program's is the tarball's own hash. `deps` lists the runtime dependencies (`PORT_RDEPS`), comma separated, each at its version (`x11-libs@88095b575f5d`), or `-` for none. A dependency is packed before what needs it. `cargo run -- packages` refuses a dependency that is not a package with files, or a loop. |
| `<arch>-boot.txt` | the boot files of the release, for `get-myos --upgrade` and `--install` (`docs/install.md`): the index's header, then `kernel <size> <sha256> <arch>-kernel`, `initramfs <size> <sha256> <arch>-initramfs`, and `esp <path> <size> <sha256> <arch>-esp-<path>` for each of Limine's files on a boot disk's ESP (`EFI/BOOT/BOOTX64.EFI`, `boot/limine/limine.conf` for slot `a`, ...). `--install` writes these, and `--upgrade` brings a disk's files to them. Every initramfs carries the same `esp` lines and files at `/lib/myos-boot/` (`--install --local`). |
| `<arch>-kernel`, `<arch>-initramfs` | the kernel as the boot disk has it (its loadable segments), and the initramfs of a default build (no Linux layer, whatever the build that wrote them has) |
| `<arch>-packages.txt` | the names of the ports the image does not carry (`packages/`): what there is to install. The index also has the image's ports, whose tarballs test the mechanism. |

A mirror is any HTTP server with these files in one directory. The names
are flat because GitHub release assets allow only that.

The image has the same header's fields in `/lib/myos-release`
(`release=... commit=... abi=...`). They come from the checkout and
`kernel/src/user/syscall.rs` when the x86_64 image is built, and are kept
in `target/myos-release`. `get-myos` compares the index with them.

## In CI

A test run that skips `net` stays network-free. One that does not
(`cargo run -- test`, `docs/testing.md`):

1. packs this build's packages for its arch;
2. serves `target/packages/` on the host's 127.0.0.1:8765. The guest
   reaches it as `http://10.0.2.2:8765` on QEMU's user network.
   `index.txt` and `packages.txt` there are the arch's;
3. as its first test, when a `pkg` test is selected, installs **the
   packages the selected tests use** into `/tmp/apps` (`MYOS_APPS`). The
   test disk's 64 MiB data partition is too small for os-test.

The packages the selected tests use are those their tags name. A
package's tests carry its name. A test of the kernel or of another port
names what it runs: `alpine_jq` names `get-alpine`, `heap` names `git`
and `coreutils`, the X clients name `tinyx`, get-myos's listing test
names `st`. A test that names none uses the mirror alone.

`install_packages` in `user/tests/run.sh` installs first one of them with
its dependencies alone, checking they came with it, then the others. In
CI, a machine's `pkg` boot installs the packages with tests and what they
use, the os-test boot installs os-test with `make`, and the Alpine boot
`get-alpine` (`docs/testing.md` "In CI").

The tests that use a package run it with `run-myos`: git and coreutils in
`heap`, os-test with `make`, and the packages' own `test.sh`, which
`run.sh` finds in the apps.

get-myos's own test (`user/get-myos/test.sh`) then checks:

- the listing;
- an app installed in the default root (`/data/apps`) that runs and is
  still there after the reboot (`app_kept`), and `apps_synced`: the
  `--sync` that `/etc/rc` ran for its list changed nothing;
- the versions of an app (`get_myos_versions`): lua at a made-up older
  version, upgraded beside it by `-u`, either one run, a rollback, `-u`
  flipping forward without a download, and `--gc` removing the old one;
- the set of apps in a root of its own (`apps_set`): exact dependency
  versions, `apps.list`, `-d` refusing a needed app, `--rollback` and
  `--switch` of the whole set, `--sync` to a shorter list, `--gc`, and
  the remembered mirror;
- global apps (`apps_global`): binds made, refused for a path another app
  has, following a rollback, and taken away by `-d` and `--unglobal`;
- `app_global` after the reboot: `clear`, made global on `/data/apps` by
  `apps_data`, bound again by `/etc/rc`.

The boot job needs `gzip` and `sha256sum`. The build job writes the
packages of the three arches (`cargo run -- packages`) and uploads them
as the `myos-packages` artifact (kept 7 days).

## Publishing

On master (a push, or the daily scheduled full boot), once the build, the
boots and the ISO passed, the `publish` job of `ci-runtime.yml` uploads
`target/packages/*` and `myos-x86_64.iso` to the rolling GitHub release
**`rolling`**. It is a prerelease, and `--clobber` replaces the files.
The job moves the release's tag to the published commit. That release is
get-myos's default mirror
(`https://github.com/davide-matasci/myos/releases/download/rolling`).

The copyleft ports (git, lynx, GNU make, ...) are redistributed there as
binaries. The tag names the commit they were built from, whose
`versions.env` pins and `*.myos.patch` files are the corresponding source
(`THIRD_PARTY_NOTICES.md`, "License compliance" in `AGENTS.md`).

## What is a package today

Everything a boot needs stays in `ports/`, with zlib and LibreSSL, which
get-myos and curl link. The ISO (`cargo run -- iso`) carries the image
only. The full test list installs the packages its tests run. It installs
none of the build dependencies, which carry no files.

`packages/` holds:

| package | what |
|---|---|
| git, lua, lynx, make, vim | the programs |
| coreutils | uutils coreutils, one multicall ELF under `bin/coreutils` |
| os-test | the POSIX conformance tests |
| x11-libs | the X client libraries for the X packages, with libX11's error database and their test |
| tinyx | the X server, `Xfbdev` |
| x11-xft | FreeType, fontconfig and Xft for the X clients |
| x11-fonts | DejaVu Sans Mono, and DejaVu Sans for what it lacks |
| dwm | the window manager |
| st | the terminal |
| dmenu | the menu |
| x11-apps | xev, xprop |
| bottom | `btm`, the system monitor |
| clear | ncurses' terminal-clearing program |
| get-alpine | the Linux layer's Alpine package installer (`docs/linux-compat.md`) |
| nginx | the web server (`packages/nginx/README.md`) |
| openssl | LibreSSL's command (`packages/openssl/README.md`) |
| acme | acme.sh, the ACME client: Let's Encrypt certificates for nginx (`packages/acme/README.md`) |
| python | CPython 3.13 against mlibc (`packages/python/README.md`) |
| glib | GLib and GIO against mlibc for WebKitGTK's stack: the `gio` command and the library's smoke (`packages/glib/README.md`) |
| image-libs | libpng, libjpeg-turbo, libwebp and pixman against mlibc for the same stack: their smoke (`packages/image-libs/README.md`) |
| cairo-pango | fribidi, HarfBuzz, cairo, pango, gdk-pixbuf and libepoxy against mlibc for the same stack: their smoke (`packages/cairo-pango/README.md`) |
| gtk3 | GTK 3 and ATK against mlibc, the toolkit of WebKitGTK and surf: its smoke window (`packages/gtk3/README.md`) |
| libsoup | libxml2, libpsl, nghttp2, libsoup 3 and glib-networking over LibreSSL against mlibc, the HTTP client of the same stack: its smoke (`packages/libsoup/README.md`) |
| webkitgtk | WebKitGTK, with ICU, libgcrypt and libtasn1, against mlibc, the web engine of surf: its smoke, a web view that loads a page (`packages/webkitgtk/README.md`) |
| surf | suckless's web browser on WebKitGTK: `run-myos surf URL` in an X session (`packages/surf/README.md`) |
| rust | `rust setup` and cargo for myos programs, in myos: the pinned nightly under the Linux layer (`packages/rust/README.md`) |
| tcc | TinyCC with newlib's headers and libraries: compiling C on the running system, `run-myos tcc` |
| ncurses, pcre2, sqlite, libedit | build dependencies, nothing in the image: ncurses of vim, lynx, clear, python and libedit; pcre2 of nginx, ripgrep and glib; sqlite of python, libsoup, webkitgtk and surf; libedit of python |
