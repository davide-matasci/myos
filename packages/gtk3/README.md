# gtk3

[GTK 3](https://gitlab.gnome.org/GNOME/gtk) (its X11 backend) and
[ATK](https://gitlab.gnome.org/GNOME/atk), static against mlibc, what
WebKitGTK and surf link (issue #616). One prefix per arch,
`target/gtk3-mlibc-<arch>` (`lib/` with the archives and their pkg-config
files, `include/`).

`build.sh` builds both with meson, cross (`myos_mlibc_meson_cross` and
`myos_mlibc_meson_build`, as `packages/cairo-pango`), over the mlibc builds
of GLib (`packages/glib`, whose host build gives `glib-compile-resources`,
`glib-mkenums` and `glib-genmarshal`), the image libraries, the text and
drawing libraries (`packages/cairo-pango`) and the X libraries and font
stack (`packages/x11-xft`). ATK is the standalone 2.38 release:
at-spi2-core, where ATK lives now, builds it only beside the AT-SPI D-Bus
pieces.

What myos lacks is left out:

- the input methods are built in (myos has no `dlopen`), and there are no
  print backends: they are modules, and `gtk-print-backends.myos.patch`
  lets the build go without the file backend GTK otherwise insists on;
- no AT-SPI bridge: `gtk-atk-bridge.myos.patch` makes at-spi2-atk optional
  for the X11 backend (it needs D-Bus), so accessibility stays inside the
  process;
- no Wayland or Broadway backend, no colord, cloudproviders, tracker,
  sysprof or introspection, no demos, examples or tests.

The app is the package's smoke, `gtk3-smoke` (`smoke.c`, its test, with
TinyX, x11-xft's `fonts.conf` and the x11-fonts package): on Xfbdev a
window holds a label, a button and an entry; once it has drawn, the button
is clicked, text goes into the entry and is read back, and the label's
pixels are read from the window.

GTK and ATK are LGPL (`THIRD_PARTY_NOTICES.md`); the smoke links them
statically.
