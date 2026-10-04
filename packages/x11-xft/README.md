# The Xft font stack for myos

Client-side fonts for X programs: [FreeType](https://freetype.org/)
rasterizes TrueType fonts, [fontconfig](https://www.freedesktop.org/wiki/Software/fontconfig/)
finds them by name (`monospace:size=10`), and libXft draws antialiased text
through the server's RENDER extension (libXrender). This is what dwm, st and
dmenu expect upstream; the fonts are the `x11-fonts` package.

```sh
get-myos x11-libs tinyx x11-xft x11-fonts
fc-match monospace        # DejaVuSansMono.ttf: "DejaVu Sans Mono" "Book"
fc-list
```

## What it is built from

| Component | Pin | License |
|-----------|-----|---------|
| expat (XML, fontconfig's configuration) | `versions.env` | MIT |
| FreeType | `versions.env` | FreeType License (FTL; its GPL-2.0 alternative is not used) |
| fontconfig | `versions.env` (2.15.0, the last with an autotools build) | MIT-style (HPND) |
| libXrender, libXft | `versions.env` | MIT |

All static, on top of a copy of x11-libs' stage: `target/x11-xft-<arch>`
holds both, as if at `/lib/x11`, for the packages that link Xft (dwm). Built
through the same cross `cc` as the X libraries (`myos_write_cross_cc` in
`scripts/myos-c-userspace-lib.sh`). FreeType without compressed fonts,
PNG glyphs or HarfBuzz.

| File | What |
|------|------|
| `fonts.conf` | fontconfig's configuration, `/lib/x11/etc/fonts/fonts.conf`: the fonts in `/lib/fonts`, the cache in `/tmp/fontconfig`, `monospace`, `sans-serif` and `serif` all DejaVu Sans Mono |
| `gperf-lite.py` | a stand-in for gperf (not a build prerequisite), enough for fontconfig's object-name table: a linear search instead of a perfect hash |
| `fontconfig-double.myos.patch` | two `long double` literals made `double`: aarch64 and riscv64 have no quad-float helpers here |
| `myos_compat.h` | `lstat`, which newlib declares elsewhere |

expat gets its hash salt from `/dev/urandom` (`XML_DEV_URANDOM`), and
fontconfig's random numbers come from `rand_r` (newlib has no
`initstate`).

## The test

`test.sh` (full mode, after the install, with `x11-fonts` and `tinyx`):
`fc-match monospace` names DejaVu Sans Mono; then on `Xfbdev :0`,
`xft_smoke` opens it through fontconfig, draws white text with Xft and reads
it back from the framebuffer: lit pixels in the text's box, with more than
two levels of grey (antialiasing).
