# cairo-pango

The text and drawing libraries of GTK 3's stack (issue #616), static
against mlibc, in one prefix per arch, `target/cairo-pango-mlibc-<arch>`
(`lib/` with the archives and their pkg-config files, `include/`):

| Library | Notes |
|---------|-------|
| [fribidi](https://github.com/fribidi/fribidi) | the Unicode bidirectional algorithm, for pango |
| [HarfBuzz](https://harfbuzz.github.io/) | shaping, over FreeType and GLib's Unicode tables; C++, against libc++ (`toolchain/libcxx`); no ICU, no hb-cairo, no tools |
| [cairo](https://cairographics.org/) | image surfaces, Xlib with RENDER (GTK 3's X backend), PNG, FreeType and fontconfig; no xcb (`packages/x11-libs` builds libxcb's core alone); taken under the LGPL-2.1 |
| [pango](https://gitlab.gnome.org/GNOME/pango) | 1.56, the last series over fontconfig 2.15 (`packages/x11-xft`'s); its cairo and FreeType backends, no Xft backend |
| [gdk-pixbuf](https://gitlab.gnome.org/GNOME/gdk-pixbuf) | its loaders built in (myos has no `dlopen`): PNG, JPEG, GIF and the others but TIFF; formats told by the loaders' signatures (no shared-mime-info database) |
| [libepoxy](https://github.com/anholt/libepoxy) | GTK 3 links it; with no GL library to load, `epoxy_has_glx` says no. Its EGL dispatch is built over the Khronos EGL headers (EGL-Registry, pinned file by file in `versions.env`), installed in the prefix for WebKitGTK, which compiles against them and never calls EGL (`packages/webkitgtk`); their native types are the opaque ones (`EGL_NO_PLATFORM_SPECIFIC_TYPES`) |

`build.sh` builds each with meson, cross, through the cross compilers of
`myos_write_cross_cc` and `myos_write_cross_cxx`, over the mlibc builds of
GLib (`packages/glib`, whose host build gives `glib-compile-resources`),
the image libraries (`packages/image-libs`), and the X libraries and font
stack (`packages/x11-xft`'s mlibc stage, its pkg-config files rewritten
from `/lib/x11` to the stage). `prefer_static` makes meson ask pkg-config
for the static libraries' own dependencies, so its link checks pass.

The app is the package's smoke, `cairo-pango-smoke` (`smoke.c`, its test,
with x11-xft's `fonts.conf` and the x11-fonts package): fribidi reorders a
mixed line, HarfBuzz shapes a word, pango lays out a line that cairo draws
into a PNG, which gdk-pixbuf loads back (and as a JPEG through memory);
then, on Xfbdev, cairo's Xlib surface fills a rectangle that `XGetImage`
reads back.

fribidi, cairo, pango and gdk-pixbuf are LGPL (`THIRD_PARTY_NOTICES.md`);
the smoke links them statically.
