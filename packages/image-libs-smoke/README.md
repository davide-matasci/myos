# image-libs-smoke

The image libraries' program, `image-smoke` (`smoke.c`, the package's
test), against mlibc, linked from `packages/image-libs`' prefix and
x11-libs' mlibc build: pixman draws an image, libpng and libwebp
round-trip it exactly, libjpeg-turbo a gradient within its loss; then, on
Xfbdev, the image goes up over MIT-SHM, RENDER fills beside it, Xcursor
makes an ARGB cursor and `XGetImage` reads the pixels back. It also asks
the server for the other extensions whose client libraries x11-libs
builds (XFIXES, RandR, XInput 2, Xinerama, Composite, DAMAGE).

```sh
get-myos tinyx image-libs-smoke
run-myos tinyx:Xfbdev :0 -br &
run-myos image-libs-smoke:image-smoke
```

It is a package of its own, not image-libs', so that a change to what it
links and the libraries never compile against, the libc's C first,
relinks it and rebuilds no library (`docs/ports.md` "Cache keys").
