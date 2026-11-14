# webkitgtk

[WebKitGTK](https://webkitgtk.org/) 2.54, the `webkit2gtk-4.1` API (GTK 3
and libsoup 3), static against mlibc, what surf links (issue #616), with
the libraries it needs beyond the other packages': [ICU](https://icu.unicode.org/)
(its data a file beside the program, `/lib/webkitgtk/icudt79l.dat` in
the programs' apps: in a program it would take it past the 128 MiB a file in
tmpfs may hold, issue #338), [libgpg-error](https://gnupg.org/software/libgpg-error/),
[libgcrypt](https://gnupg.org/software/libgcrypt/) (WebKit's digests and
ciphers), [libtasn1](https://www.gnu.org/software/libtasn1/) and
HarfBuzz's ICU library (`packages/cairo-pango`'s HarfBuzz, built again
with it). One prefix per arch, `target/webkitgtk-mlibc-<arch>` (`lib/`
with the archives and their pkg-config files, `include/`). WebKit is one
archive, `libwebkit2gtk-4.1.a` (JavaScriptCore with it), which a program
links with `--gc-sections` as WebKit links its own (its `.pc` says so):
code nothing calls refers to symbols nothing defines.

`build.sh` builds ICU's tools for the build host first (a cross build of
ICU runs them), then per arch the libraries with their own builds
(autotools, meson) and WebKit with CMake, over the mlibc builds of GLib,
the image, text and X libraries, GTK 3 (`packages/gtk3`) and libsoup
(`packages/libsoup`). A WebKit build takes hours: CI builds an arch per
job (`PORT_ARCH_JOBS`, `docs/ports.md`), `MYOS_WEBKITGTK_ARCHES` names the
arches to build (all by default), and a failed run's CMake tree is kept,
so the next run goes on where it stopped.

WebKit is built as for Linux, its best-trodden road: CMake's system is
Linux and its sources see `__linux__`, against mlibc, which is
Linux-shaped. What myos lacks is taken away:

- no EGL or GL: WebKit draws with Skia on the CPU into shared memory
  that the browser's process puts on the screen, its path for machines
  where GL fails. The patch (`webkitgtk-myos.myos.patch`, all under
  `WEBKIT_MYOS`) says so where WebKit would ask EGL (no hardware
  acceleration, no EGL display, no accelerated buffers): libepoxy would
  need to load an EGL library, and myos has no `dlopen`. The Khronos EGL
  headers WebKit compiles against are cairo-pango's;
- one binary: WebKit's web and network processes are the program itself
  (`Source/WebKit/myos/MultiCall.cpp`, added by the patch), which the
  browser's process runs again with the process's name first; a program
  links the archive with its `main` renamed `webkit_myos_program_main`,
  which MultiCall.cpp's `main` calls; GTK's injected bundle is in it too,
  called by the web process instead of loaded;
- no Linux system calls: `gettid` is the libc's, and `compat/` has the
  Linux headers mlibc lacks (`prctl`, `sysinfo`, `mincore`,
  `malloc_trim`, `fallocate`, `sys/syscall.h`, the ARM64 JIT's
  `asm/hwcap.h`: no optional instructions), their calls failing or
  answering from `sysconf`; the memory pressure monitor asks `sysconf`
  for the machine's memory and what is available of it (the patch:
  myos's `/proc/meminfo` is not Linux's, and there are no zones or
  cgroups); the allocator is mlibc's malloc (`USE_SYSTEM_MALLOC`):
  WebKit's own (bmalloc, libpas) are made of Linux system calls;
- on riscv64, JavaScriptCore's C interpreter (CLoop): no JIT and no
  WebAssembly, which WebKit 2.54 does not build there (the in-place
  WebAssembly interpreter has no RISC-V code), and a JIT would need every
  hart's instruction cache flushed (issue #652);
- WebAssembly's memory is bounds-checked instead of caught faulting
  (the patch turns `useWasmFaultSignalHandler` off: fast memory reserves
  gigabytes of address space a memory, which has not been tried on
  myos). The rest of the machine context is upstream's: a signal
  handler's `ucontext_t` (`docs/signals.md`) gives a suspended thread's
  registers to the collector and to the sampling profiler;
- no translations (GLib has none), and none of the media stack
  (GStreamer), WebGL, the GPU process, the sandbox, WebDriver, spell
  checking, speech, gamepads, introspection, the optional image and font
  formats (AVIF, JPEG XL, WOFF2, color management) or libsecret.

No app here: WebKit's programs are packages of their own, linked from
the archive, `webkitgtk-smoke` (WebKit's program and boot test,
`packages/webkitgtk-smoke`) and surf. WebKit's own build links a stub,
`program.c` (`WEBKIT_MYOS_PROGRAM`), only for the list of what the
archive gathers. What the programs link and WebKit never compiles
against, GIO's TLS backend and LibreSSL, is so theirs to link: a change
to glib-networking or LibreSSL relinks them and leaves WebKit's build
alone. WebKit is a library port: its cache key takes the compile
flavours of what it compiles against, the libc's headers and symbols
among them, not the archives a program links, so a fix in the libc's C
relinks the programs and leaves the build alone too (`docs/ports.md`
"Cache keys", issues #664 and #773). ICU's data, `share/icu-data`, is
shipped by each program's package.

WebKit's WebCore, libgpg-error, libgcrypt and libtasn1 are LGPL
(`THIRD_PARTY_NOTICES.md`); the programs link them statically.
