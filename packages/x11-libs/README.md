# X client libraries for myos

The libraries an X client links, built from the X.Org release tarballs
(fetched, not vendored; pins in `versions.env`):

| Component | Why |
|-----------|-----|
| xorgproto | the protocol headers (`X11/X.h`, `Xproto.h`, ...) |
| xtrans | the transport code libX11 compiles in |
| libXau | X authority files (`~/.Xauthority`) |
| xcb-proto | the protocol in XML and its Python generator, used at build time |
| libxcb | the protocol library, core only (no extension libraries) |
| libX11 | Xlib, over libxcb; thread-safe after `XInitThreads()`, no XKB |
| libXext | the common extensions' client side: MIT-SHM (`XShm*`, on libgloss's System V shared memory), SHAPE, SYNC, ... |

All static, for the three arches, under `target/x11-libs-<arch>` as if
installed at `/lib/x11` (libX11's data at `/lib/X11`). An X package builds
against them with

```sh
export PKG_CONFIG_SYSROOT_DIR=$ROOT/target/x11-libs-$arch
export PKG_CONFIG_LIBDIR=$PKG_CONFIG_SYSROOT_DIR/lib/x11/lib/pkgconfig:$PKG_CONFIG_SYSROOT_DIR/lib/x11/share/pkgconfig
```

and `PORT_DEPS="x11-libs"` in its `port.env`.

## How it builds

Each component's own autoconf `configure`, in cross mode (`--build` the
host, `--host=<arch>-unknown-elf`), out of tree in `target/x11-libs-build`.
The compiler is a wrapper `build.sh` writes: clang against the newlib
sysroot, and for a link `ld.lld` with crt0, libc and libgloss, the way the C
smokes link, so configure's link tests answer for myos. `myos_compat.h`
(forced in with `-include`) and `include/netinet/tcp.h` fill in what the X
code expects from libc headers libgloss lacks. libxcb asks pkg-config for
`pthread-stubs`, which is an empty `.pc` on a libc with the pthread
functions; `build.sh` writes it.

## What the package installs

- `/lib/X11/XErrorDB`: the error messages `XGetErrorText` prints.
- `/lib/X11/locale`: libX11's locale data for the C locale (`locale.alias`,
  `locale.dir`, `compose.dir`, `C/XLC_LOCALE` and its Compose file,
  `iso8859-1/Compose`). `XOpenIM` opens libX11's own input method only when
  the locale's Compose file exists (with no `XMODIFIERS`, the default), and
  fails otherwise: there is no input method server. dmenu needs it; st
  retries with `@im=local`.
- `/bin/etc/x11_smoke`: the boot test (`test.sh`, full mode). With no X
  server on myos yet, it is both ends: a stand-in server on
  `/tmp/.X11-unix/X5` that answers the connection setup and the handful of
  requests `XOpenDisplay` and a window's round trip send, and a libX11
  client that checks the screen it was given and creates, names and maps a
  window, which the server checks it received. Then four of the client's
  threads intern 50 atoms each on its one display at once, after
  `XInitThreads()`: 200 atoms, all different, each reply to the thread
  that asked.

## Not yet

- The other locales' data (`/lib/X11/locale`): nothing on myos sets a
  locale other than C (no `LANG`).
- IPv6 displays, XKB.
- `XInitThreads()` at load: Xlib's locks are on libgloss's pthreads
  (`docs/threads.md`), but upstream's constructor that turns them on by
  itself is built out, as myos's crt0 runs no constructors (issue #343). A
  threaded client calls `XInitThreads()` first, as before Xlib 1.8.
