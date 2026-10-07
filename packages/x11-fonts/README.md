# Fonts for X clients

[DejaVu Sans Mono](https://dejavu-fonts.github.io/) 2.37 (regular, bold,
oblique, bold oblique) in `/lib/fonts`, where the `x11-xft` package's
fontconfig looks, and its license as `/lib/fonts/LICENSE.DejaVu`. It is
fontconfig's `monospace`, `sans-serif` and `serif` for now.

DejaVu Sans (regular) is there for the characters the Mono faces lack:
fontconfig falls back to it, and st draws them with it. Among them are
the braille patterns (U+2800–28FF), which terminal programs use as 2×4
dots per cell for graphs (bottom's `btm`).

`build.sh` takes the five TTFs and the license out of the pinned release
tarball (`versions.env`), with python3's `tarfile` (the tarball is
`.tar.bz2`, and bzip2 is not a build prerequisite).

License: the Bitstream Vera Fonts license, and the Arev Fonts license (its
counterpart) for the glyphs DejaVu took from Arev; DejaVu's own changes are
public domain. Both permissive: use, modification and redistribution,
keeping the copyright and license notices, which `LICENSE.DejaVu` carries
next to the fonts.

The `x11-xft` package's test checks that fontconfig finds these fonts and
that Xft draws with them, and that a braille pattern resolves to DejaVu
Sans.
