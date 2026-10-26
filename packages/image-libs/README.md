# image-libs

The image libraries of WebKitGTK's stack (issue #616), static against
mlibc, in one prefix per arch, `target/image-libs-mlibc-<arch>` (`lib/`
with the archives and their pkg-config files, `include/`):

| Library | Notes |
|---------|-------|
| [libpng](http://www.libpng.org/pub/png/libpng.html) | over `ports/zlib`'s mlibc build (copied in, with its `zlib.pc`); NEON on aarch64, no RISC-V vector code |
| [libjpeg-turbo](https://libjpeg-turbo.org/) | the 8-bit `libjpeg` API alone (no TurboJPEG API, no tools), no SIMD (its x86 assembly needs nasm) |
| [libwebp](https://developers.google.com/speed/webp) | `libwebp`, `libsharpyuv`, `libwebpmux`, `libwebpdemux`, `libwebpdecoder`; no tools |
| [pixman](https://pixman.org/) | meson, cross (as `packages/glib`); SSE2/SSSE3 (checked at run time) and NEON, no RISC-V vector code |

`build.sh` builds libpng and libwebp with their own `configure`,
libjpeg-turbo with CMake and pixman with meson, all with the cross cc of
`myos_write_cross_cc`.

The app is the package's smoke, `image-smoke` (`smoke.c`, its test): pixman
draws an image, libpng and libwebp round-trip it exactly, libjpeg-turbo a
gradient within its loss; then, on Xfbdev, through `packages/x11-libs`'
mlibc build, the image goes up over MIT-SHM, RENDER fills beside it,
Xcursor makes an ARGB cursor and `XGetImage` reads the pixels back. It
also asks the server for the other extensions whose client libraries
x11-libs builds (XFIXES, RandR, XInput 2, Xinerama, Composite, DAMAGE).
