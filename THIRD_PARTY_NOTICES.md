# Third-party notices

myos's **own code** is licensed under either of

- Apache License 2.0 ([`LICENSE-APACHE`](LICENSE-APACHE))
- MIT license ([`LICENSE-MIT`](LICENSE-MIT))

at your option. Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in myos by you, as defined in the
Apache-2.0 license, shall be dual licensed as above, without any additional
terms or conditions.

This repository **does not vendor** upstream projects: the build scripts fetch
pinned upstream sources (see the `versions.env` / `fetch.sh` in each
`ports/<name>/` directory) and apply the patches and shims kept here. Those
patches, and any file here that is derived from an upstream file, stay under
**the upstream project's license** (listed below), not the dual license above.
Likewise, a built myos image or ISO is a collection of separate programs, each
under its own license.

## Components in the default boot image / ISO

| Component | Pinned version | License | Source |
|-----------|----------------|---------|--------|
| Limine bootloader (BIOS/UEFI binaries, and the `limine` tool built from the release's `limine.c` with the BIOS stage it embeds, at `/bin/etc/limine`) | 12.6.1 | BSD-2-Clause | https://github.com/limine-bootloader/limine (`ports/limine`, `src/limine_image.rs`) |
| newlib + libgloss (C library; `toolchain/newlib/`) | 4.4.0 | Mostly BSD-style permissive licenses; see `COPYING.NEWLIB` / `COPYING.LIBGLOSS` in the upstream source | https://sourceware.org/git/newlib-cygwin.git |
| mlibc (the second C library; `toolchain/mlibc/`, its sysroot at `/lib/mlibc` in the image, the `/bin/etc/mlibc/*` smokes linked against it; the checked-in ABI headers derived from its `abis/linux` ones, `toolchain/mlibc/sysdeps/myos/include/abi-bits`, and the patches of `toolchain/mlibc/patch.sh` stay under its license) | 7.0.0 | MIT (parts from musl, MIT, and from other permissive sources: `LICENSE` in the upstream source) | https://github.com/managarm/mlibc |
| LLVM's C++ runtimes: libc++, libc++abi, libunwind (`toolchain/libcxx`, built against mlibc; in the image only in the C++ smoke, `/bin/etc/mlibc/cxx_smoke`, and in the C++ programs that link them) | `LLVM_TAG` in `toolchain/libcxx/versions.env` | Apache-2.0 WITH LLVM-exception (`LICENSE.TXT` in the upstream source; the exception waives the attribution a binary would owe) | https://github.com/llvm/llvm-project |
| frigg (mlibc's C++ utility library, built into mlibc; `FRIGG_REV` in `toolchain/mlibc/versions.env`) | pinned revision | MIT | https://github.com/managarm/frigg |
| libsmarter (built into mlibc; `LIBSMARTER_REV`) | pinned revision | MIT | https://github.com/managarm/libsmarter |
| freestnd-c-hdrs, freestnd-cxx-hdrs (GCC's freestanding C and C++ headers, which mlibc's build compiles against; build time only, nothing of them is in an image; `FREESTND_*_HDRS_REV`) | pinned revisions | **GPL-3.0-or-later WITH GCC-exception-3.1** (`COPYING3`, `COPYING.RUNTIME` in the upstream sources; `licenses/GPL-3.0.txt`) | https://github.com/osdev0/freestnd-c-hdrs, https://github.com/osdev0/freestnd-cxx-hdrs |
| Rust `std` and `core` (patched; `toolchain/std/`), linked into Rust userspace programs; their rlibs also ship in the `rust` package (`/lib/rust`) | pinned nightly (`rust-toolchain.toml`) | MIT OR Apache-2.0 | https://github.com/rust-lang/rust |
| The Rust toolchain `rust setup` installs (package `rust`: rustc with LLVM and rust-lld, cargo, the musl std; the official musl builds, sha256-pinned in `packages/rust/versions.env`) | pinned nightly (`rust-toolchain.toml`) | MIT OR Apache-2.0 (LLVM: Apache-2.0 WITH LLVM-exception); their `COPYRIGHT` files list the bundled parts | downloaded **at run time** from https://static.rust-lang.org/ on the user's machine (with Alpine's lld, by `get-alpine`); not in the image or the package |
| dlmalloc (the Rust crate: a port of Doug Lea's malloc, public domain), the heap of the myos `std` (`toolchain/std/sys/myos/alloc.rs`), linked into Rust userspace programs | 0.2.13 (as `std`'s `Cargo.lock` pins it) | MIT OR Apache-2.0 | https://github.com/alexcrichton/dlmalloc-rs |
| Rust crates in the kernel / `user/netd` | see `Cargo.toml` | `limine`: MIT OR Apache-2.0; `spin`: MIT; `x86_64`, `linked_list_allocator`, `pic8259`: MIT/Apache-2.0; `smoltcp`: 0BSD; `fdt` (device tree parser, used unmodified): **MPL-2.0** (`licenses/MPL-2.0.txt`); `ruzstd` (the boot archives' zstd decoder, used unmodified) and its `twox-hash`: MIT | https://crates.io |
| Rust crates in the kernel / `user/netd` | see `Cargo.toml` | `limine`: MIT OR Apache-2.0; `spin`: MIT; `x86_64`, `linked_list_allocator`, `pic8259`: MIT/Apache-2.0; `smoltcp`: 0BSD (netd's carries myos patches, `user/net/smoltcp/*.myos.patch`, under the same license); `fdt` (device tree parser, used unmodified): **MPL-2.0** (`licenses/MPL-2.0.txt`); `ruzstd` (the boot archives' zstd decoder, used unmodified) and its `twox-hash`: MIT | https://crates.io |
| The `iwx` crate (`user/wifi/iwx`, the AX201's and AX200's driver in the `wifi` daemon): a port of OpenBSD's iwx(4) driver (`if_iwx.c` rev 1.230, `if_iwxreg.h` rev 1.75, `if_iwxvar.h` rev 1.50) to Rust | OpenBSD CVS revisions above | **ISC** (genua, Stefan Sperling, Damien Bergamini, Fixup Software) **AND BSD-3-Clause** (Intel's part, dual BSD/GPLv2, taken under the BSD licence); notices in `user/wifi/iwx/LICENSE` | https://cvsweb.openbsd.org/src/sys/dev/pci/if_iwx.c |
| The `mtw` crate (`user/wifi/mtw`, the MT7601U's driver in the `wifi` daemon): a port of OpenBSD's mtw(4) driver (`if_mtw.c` rev 1.13, `if_mtwvar.h` rev 1.1, `mtwreg.h` rev 1.2) and its AMRR rate adaptation (`ieee80211_amrr.c` rev 1.12) to Rust; its USB ids from OpenBSD's `usbdevs` | OpenBSD CVS revisions above | **ISC** (Damien Bergamini, Kevin Lo, James Hastings); notices in `user/wifi/mtw/LICENSE` | https://cvsweb.openbsd.org/src/sys/dev/usb/if_mtw.c |
| The `mtw` crate's `mt76x2` (`user/wifi/mtw/src/mt76x2`, the MT7612U's driver): a port of Linux's mt76x2u driver and the mt76x02 and mt76 code it uses (`drivers/net/wireless/mediatek/mt76`: `mt76x2/usb*.c` and the files `user/wifi/mtw/LICENSE` lists) to Rust; its USB ids from Linux's `mt76x2u_device_table` | Linux `45f9960f8cf3` | **BSD-3-Clause-Clear** (Felix Fietkau, Lorenzo Bianconi, Stanislaw Gruszka); text and notices in `user/wifi/mtw/LICENSE` | https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/drivers/net/wireless/mediatek/mt76 |
| The `bwfm` crate (`user/wifi/bwfm`, the Raspberry Pi's CYW43455's and CYW43438's driver in the `wifi` daemon, over `/dev/sdio`): a port of OpenBSD's bwfm(4) driver and its SDIO attachment (`bwfm.c` rev 1.118, `bwfmreg.h` rev 1.28, `bwfmvar.h` rev 1.32, `if_bwfm_sdio.c` rev 1.45, `if_bwfm_sdio.h` rev 1.3) to Rust | OpenBSD CVS revisions above | **ISC** (Broadcom Corporation, Patrick Wildt); notices in `user/wifi/bwfm/LICENSE` | https://cvsweb.openbsd.org/src/sys/dev/ic/bwfm.c |
| Rust crates in the `wpa` crate (`user/wifi/wpa/Cargo.toml`: WPA2-PSK in the `wifi` daemon, and the launcher's simulated access points) | `aes` 0.8.4, `aes-kw` 0.2.1, `ccm` 0.5.0, `hmac` 0.12.1, `pbkdf2` 0.12.2, `sha1` 0.10.6 (RustCrypto, used unmodified) | those and their dependencies `aead`, `block-buffer`, `cfg-if`, `cipher`, `cpufeatures`, `crypto-common`, `ctr`, `digest`, `inout`, `typenum`: MIT OR Apache-2.0; `generic-array`: MIT; `subtle`: BSD-3-Clause | https://github.com/RustCrypto |
| Rust crates in the `xhci` module (`modules/xhci/Cargo.toml`) | `xhci` 0.9.2 | `xhci` (register, TRB and context layouts, used unmodified) and its dependencies `accessor`, `bit_field`, `num-derive`, `num-traits`, `paste`: MIT OR Apache-2.0 | https://github.com/rust-osdev/xhci |
| Rust crates in the `sdmmc` module (`modules/sdmmc/Cargo.toml`) | `sdio-host` 0.9.0 | `sdio-host` (the SD commands and the card registers' layouts, used unmodified): MIT OR Apache-2.0; no dependencies | https://github.com/jkristell/sdio-host |
| Rust crates in the virtio modules (`modules/virtio/Cargo.toml`) | `virtio-drivers` 0.13.0 | `virtio-drivers` (virtqueues, the PCI and MMIO transports, the block and net devices, used unmodified): MIT; its dependencies `bitflags`, `enumn`, `log`, `safe-mmio`, `thiserror`: MIT OR Apache-2.0; `zerocopy`: BSD-2-Clause OR Apache-2.0 OR MIT | https://github.com/rcore-os/virtio-drivers |
| Rust crates in the `acpi` module (`modules/acpi/Cargo.toml`, x86_64 only) | `acpi` 6.1.1 | `acpi` (the ACPI tables and the AML interpreter, used unmodified) and its dependencies `bit_field`, `pci_types`, `spinning_top`: MIT/Apache-2.0; `bitflags`, `log`, `lock_api`, `scopeguard`: MIT OR Apache-2.0; `byteorder`: Unlicense OR MIT (taken as MIT) | https://github.com/rust-osdev/acpi |
| Rust crates in the `fat` module (`modules/fat/fatvol/Cargo.toml`) | `fstool` 0.4.35 (the FAT driver only, used unmodified) | `fstool`: MIT; its dependency `uuid`: MIT OR Apache-2.0 | https://github.com/KarpelesLab/fstool |
| sbase | `SBASE_REV` in `ports/sbase/versions.env` | MIT | https://git.suckless.org/sbase |
| ubase | `UBASE_REV` in `ports/ubase/versions.env` | MIT | https://github.com/michaelforney/ubase |
| uutils coreutils (+ its Rust dependencies) | 0.10.0 (see `packages/coreutils/README.md`) | MIT (dependencies: MIT / Apache-2.0 and similar permissive licenses) | https://github.com/uutils/coreutils |
| ripgrep (+ its Rust dependencies) | 15.2.0 | Unlicense OR MIT | https://github.com/BurntSushi/ripgrep |
| oksh (`/sh`) | 7.9 | Public domain (core) + BSD/ISC (portability files) | https://github.com/ibara/oksh |
| Dropbear SSH | 2026.94 | MIT-style (plus BSD / public-domain parts; see upstream `LICENSE`; the myos patch, `ports/dropbear/*.myos.patch`, under the same license) | https://matt.ucc.asn.au/dropbear/ |
| TinyCC (package `tcc`, with newlib's sysroot) and `libtcc1` | `TCC_REV` in `packages/tcc/versions.env` | **LGPL-2.1** | https://github.com/TinyCC/tinycc |
| Vim | 9.2.0 | Vim license (GPL-compatible, "charityware") | https://github.com/vim/vim |
| ncurses | 6.5 | X11/MIT-style | https://ftp.gnu.org/gnu/ncurses/ |
| SQLite (package `sqlite`, linked into python) | `SQLITE_VERSION` in `packages/sqlite/versions.env` | Public domain | https://sqlite.org/ |
| libedit (package `libedit`, linked into python) | `LIBEDIT_VERSION` in `packages/libedit/versions.env` | BSD-3-Clause | https://thrysoee.dk/editline/ |
| zlib | 1.3.1 | zlib | https://github.com/madler/zlib |
| Git | 2.55.0 | **GPL-2.0-only** (some parts LGPL-2.1+) | https://github.com/git/git |
| Lynx | 2.9.3 | **GPL-2.0-only** | https://invisible-island.net/lynx/ |
| GNU Make | 4.4.1 | **GPL-3.0-or-later** | https://ftp.gnu.org/gnu/make/ |
| Lua | 5.4.7 | MIT | https://www.lua.org/ |
| CPython (package `python`, built against mlibc; its bundled expat, libmpdec, `_hacl` and `_blake2` sources under their own permissive licenses, `Doc/license.rst` in the upstream source; the myos patches `packages/python/*.myos.patch` under the same license; `pip`, unpacked from the wheel CPython bundles, MIT, with its vendored packages under their own licenses, all in its `dist-info/licenses`: MIT, BSD, Apache-2.0 and, for `certifi` and its CA bundle, **MPL-2.0** (`licenses/MPL-2.0.txt`)) | `PYTHON_VERSION` in `packages/python/versions.env` | PSF-2.0 (the Python Software Foundation License, GPL-compatible, permissive) | https://www.python.org/ |
| nginx (package `nginx`) | `NGINX_VERSION` in `packages/nginx/versions.env` | BSD-2-Clause (the myos patches, `packages/nginx/*.myos.patch`, under the same license) | https://nginx.org/ |
| PCRE2 (`packages/pcre2`, the 8-bit library linked into ripgrep and nginx; its mlibc build into the glib package) | `PCRE2_VERSION` in `packages/pcre2/versions.env` | BSD-3-Clause WITH PCRE2-exception | https://github.com/PCRE2Project/pcre2 |
| libpng (package `image-libs`, against mlibc) | `LIBPNG_VERSION` in `packages/image-libs/versions.env` | libpng License (PNG Reference Library License version 2: permissive, `LICENSE` in the upstream source) | http://www.libpng.org/pub/png/libpng.html |
| libjpeg-turbo (package `image-libs`, against mlibc) | `LIBJPEG_TURBO_VERSION` in `packages/image-libs/versions.env` | IJG License, BSD-3-Clause and zlib (`LICENSE.md` in the upstream source; "this software is based in part on the work of the Independent JPEG Group") | https://libjpeg-turbo.org/ |
| libwebp (package `image-libs`, against mlibc) | `LIBWEBP_VERSION` in `packages/image-libs/versions.env` | BSD-3-Clause, with Google's patent grant (`COPYING`, `PATENTS` in the upstream source) | https://chromium.googlesource.com/webm/libwebp |
| pixman (package `image-libs`, against mlibc) | `PIXMAN_VERSION` in `packages/image-libs/versions.env` | MIT (`COPYING` in the upstream source) | https://pixman.org/ |
| fribidi (package `cairo-pango`, against mlibc) | `FRIBIDI_VERSION` in `packages/cairo-pango/versions.env` | **LGPL-2.1-or-later** (`licenses/LGPL-2.1.txt`) | https://github.com/fribidi/fribidi |
| HarfBuzz (package `cairo-pango`, against mlibc) | `HARFBUZZ_VERSION` in `packages/cairo-pango/versions.env` | MIT ("Old MIT", `COPYING` in the upstream source) | https://harfbuzz.github.io/ |
| cairo (package `cairo-pango`, against mlibc) | `CAIRO_VERSION` in `packages/cairo-pango/versions.env` | **LGPL-2.1** (cairo is LGPL-2.1 or MPL-1.1; taken under the LGPL-2.1, `licenses/LGPL-2.1.txt`; a few files under MIT-style licenses, `COPYING` in the upstream source) | https://cairographics.org/ |
| pango (package `cairo-pango`, against mlibc) | `PANGO_VERSION` in `packages/cairo-pango/versions.env` | **LGPL-2.0-or-later** (taken as LGPL-2.1, `licenses/LGPL-2.1.txt`) | https://gitlab.gnome.org/GNOME/pango |
| gdk-pixbuf (package `cairo-pango`, against mlibc, its loaders built in) | `GDK_PIXBUF_VERSION` in `packages/cairo-pango/versions.env` | **LGPL-2.1-or-later** (`licenses/LGPL-2.1.txt`) | https://gitlab.gnome.org/GNOME/gdk-pixbuf |
| libepoxy (package `cairo-pango`, against mlibc) | `LIBEPOXY_VERSION` in `packages/cairo-pango/versions.env` | MIT (`COPYING` in the upstream source; its generated GL dispatch from the Khronos registry, Apache-2.0) | https://github.com/anholt/libepoxy |
| ATK (package `gtk3`, against mlibc) | `ATK_VERSION` in `packages/gtk3/versions.env` | **LGPL-2.0-or-later** (taken as LGPL-2.1, `licenses/LGPL-2.1.txt`) | https://gitlab.gnome.org/GNOME/atk |
| GTK 3 (package `gtk3`, against mlibc, patched: `packages/gtk3/*.myos.patch`) | `GTK_VERSION` in `packages/gtk3/versions.env` | **LGPL-2.0-or-later** (taken as LGPL-2.1, `licenses/LGPL-2.1.txt`) | https://gitlab.gnome.org/GNOME/gtk |
| libxml2 (package `libsoup`, against mlibc, patched: `packages/libsoup/libxml2-*.myos.patch`) | `LIBXML2_VERSION` in `packages/libsoup/versions.env` | MIT (`Copyright` in the upstream source) | https://gitlab.gnome.org/GNOME/libxml2 |
| libpsl (package `libsoup`, against mlibc), with the Public Suffix List built in (`list/public_suffix_list.dat` of the same release) | `LIBPSL_VERSION` in `packages/libsoup/versions.env` | MIT; the list **MPL-2.0** (`licenses/MPL-2.0.txt`) | https://github.com/rockdaboot/libpsl |
| nghttp2 (package `libsoup`, its library, against mlibc) | `NGHTTP2_VERSION` in `packages/libsoup/versions.env` | MIT (`COPYING` in the upstream source) | https://nghttp2.org/ |
| libsoup 3 (package `libsoup`, against mlibc) | `LIBSOUP_VERSION` in `packages/libsoup/versions.env` | **LGPL-2.0-or-later** (taken as LGPL-2.1, `licenses/LGPL-2.1.txt`) | https://gitlab.gnome.org/GNOME/libsoup |
| glib-networking (package `libsoup`, its OpenSSL backend over LibreSSL, against mlibc, patched: `packages/libsoup/*.myos.patch`) | `GLIB_NETWORKING_VERSION` in `packages/libsoup/versions.env` | **LGPL-2.1-or-later** (`licenses/LGPL-2.1.txt`) with its OpenSSL linking exception (`LICENSE_EXCEPTION` in the upstream source), which covers linking LibreSSL; the myos patches extend it to the files they change | https://gitlab.gnome.org/GNOME/glib-networking |
| Khronos EGL headers (`EGL/egl.h`, `EGL/eglext.h`, `EGL/eglplatform.h`, `KHR/khrplatform.h`, from EGL-Registry; in the cairo-pango package's prefix: libepoxy's EGL dispatch and WebKitGTK compile against them) | `EGL_REGISTRY_COMMIT` in `packages/cairo-pango/versions.env` | Apache-2.0 (the `EGL/` headers; `LICENSE-APACHE`), MIT-style (`khrplatform.h`) | https://github.com/KhronosGroup/EGL-Registry |
| ICU (package `webkitgtk`, its common, i18n and data libraries, against mlibc) | `ICU_VERSION` in `packages/webkitgtk/versions.env` | Unicode-3.0 (the Unicode License v3; the data's and the third-party code's notices in `LICENSE` in the upstream source) | https://icu.unicode.org/ |
| libgpg-error (package `webkitgtk`, the library, against mlibc) | `LIBGPG_ERROR_VERSION` in `packages/webkitgtk/versions.env` | **LGPL-2.1-or-later** (`licenses/LGPL-2.1.txt`) | https://gnupg.org/software/libgpg-error/ |
| libgcrypt (package `webkitgtk`, the library, against mlibc) | `LIBGCRYPT_VERSION` in `packages/webkitgtk/versions.env` | **LGPL-2.1-or-later** (`licenses/LGPL-2.1.txt`; a few files under permissive licenses, `LICENSES` in the upstream source) | https://gnupg.org/software/libgcrypt/ |
| libtasn1 (package `webkitgtk`, the library, against mlibc) | `LIBTASN1_VERSION` in `packages/webkitgtk/versions.env` | **LGPL-2.1-or-later** (`licenses/LGPL-2.1.txt`) | https://www.gnu.org/software/libtasn1/ |
| WebKitGTK (package `webkitgtk`, against mlibc, patched: `packages/webkitgtk/*.myos.patch`, under the licenses of the files it changes; its new `Source/WebKit/myos/MultiCall.cpp` BSD-2-Clause, as WebKit's own; the compat headers `packages/webkitgtk/compat/` myos's own) | `WEBKITGTK_VERSION` in `packages/webkitgtk/versions.env` | **LGPL-2.1** and **LGPL-2.0-or-later** (WebCore, parts of WebKit; `licenses/LGPL-2.1.txt`), BSD-2-Clause (JavaScriptCore, WTF, most of WebKit), and the third-party code it builds in under its own licenses (Skia BSD-3-Clause; xdgmime AFL-2.0 or LGPL-2.0-or-later; simdutf, fast_float and the others permissive: `Source/ThirdParty/*` and the files' headers in the upstream source) | https://webkitgtk.org/ |
| GLib: GLib, GObject, GIO, GModule (package `glib`, built against mlibc and linked statically into its `gio` and `glib-smoke`; the myos patch `packages/glib/*.myos.patch` under the same license) | `GLIB_VERSION` in `packages/glib/versions.env` | **LGPL-2.1-or-later** (`licenses/LGPL-2.1.txt`; a few files under other permissive licenses, `LICENSES/` in the upstream source) | https://gitlab.gnome.org/GNOME/glib |
| libffi (in the glib package, GObject's generic marshaller) | `LIBFFI_VERSION` in `packages/glib/versions.env` | MIT | https://github.com/libffi/libffi |
| GNU libiconv (in the glib package: the library, GLib's iconv, mlibc having none) | `LIBICONV_VERSION` in `packages/glib/versions.env` | **LGPL-2.1-or-later** (the library; `licenses/LGPL-2.1.txt`) | https://www.gnu.org/software/libiconv/ |
| proxy-libintl (in the glib package: GLib's gettext, returning the message, built without its `dlopen` of a real libintl) | `PROXY_LIBINTL_REV` in `packages/glib/versions.env` | **LGPL-2.0-or-later** (taken as LGPL-2.1, `licenses/LGPL-2.1.txt`) | https://github.com/frida/proxy-libintl |
| bottom (package `bottom`, `btm`) + its Rust dependencies | `packages/bottom/versions.env` (dependencies: bottom's `Cargo.lock`) | bottom, crossterm, sysinfo, ratatui: MIT (the myos patches to them: the same); dependencies: MIT / Apache-2.0 and similar permissive licenses (Zlib, Unlicense OR MIT); `option-ext` 0.2.0 (used unmodified, through `dirs-sys`): **MPL-2.0** (`licenses/MPL-2.0.txt`) | https://github.com/ClementTsang/bottom |
| X client libraries (package `x11-libs`, built against newlib and against mlibc): xorgproto, xtrans, libXau, xcb-proto, libxcb, libX11, libXext, libXrender, libXfixes, libXi, libXrandr, libXdamage, libXcomposite, libXcursor, libXinerama | see `packages/x11-libs/versions.env` | MIT / X11-style (X.Org, The Open Group and others; each upstream `COPYING`) | https://www.x.org/releases/individual/ |
| TinyX `Xfbdev` (package `tinyx`; the X server) | `TINYX_REV` in `packages/tinyx/versions.env` | **GPL-3.0** (TinyX's changes; the X.Org code it started from is MIT/X11) | https://github.com/tinycorelinux/tinyx |
| libfontenc, libXfont 1.x, libXdmcp's header (linked into / used to build `Xfbdev`) | see `packages/tinyx/versions.env` | MIT / X11-style | https://www.x.org/releases/individual/lib/ |
| dwm (package `dwm`; the window manager) | `DWM_VERSION` in `packages/dwm/versions.env` | MIT/X Consortium | https://dwm.suckless.org/ |
| st (package `st`; the terminal), and the `st` / `st-256color` entries of `/lib/termcap` (`ports/termcap/termcap`, converted from its `st.info`, notice kept in the file) | `ST_VERSION` in `packages/st/versions.env` | MIT/X Consortium | https://st.suckless.org/ |
| dmenu (package `dmenu`; the menu, with `stest`, `dmenu_run`, `dmenu_path`) | `DMENU_VERSION` in `packages/dmenu/versions.env` | MIT/X Consortium | https://tools.suckless.org/dmenu/ |
| xev (package `x11-apps`) | see `packages/x11-apps/versions.env` | MIT/X Consortium | https://www.x.org/releases/individual/ |
| Xft font stack (package `x11-xft`, built against newlib and against mlibc): expat, FreeType, fontconfig, libXft | see `packages/x11-xft/versions.env` | expat, libXft: MIT; fontconfig: MIT-style (HPND); FreeType: the FreeType License (FTL, used instead of its GPL-2.0 alternative; credit below) | https://libexpat.github.io/ https://freetype.org/ https://www.freedesktop.org/wiki/Software/fontconfig/ https://www.x.org/releases/individual/lib/ |
| DejaVu Sans Mono and DejaVu Sans (package `x11-fonts`, `/lib/fonts`) | `DEJAVU_VERSION` in `packages/x11-fonts/versions.env` | Bitstream Vera Fonts license and Arev Fonts license (DejaVu's changes public domain); the text ships as `/lib/fonts/LICENSE.DejaVu` | https://dejavu-fonts.github.io/ |
| LibreSSL (libcrypto, libssl, libtls: TLS for `curl`, `lynx`, `nginx`, `user/tls`, and `python` from its mlibc build; the myos patch `ports/libressl/cafile.myos.patch` under the same license) | 4.3.3 | ISC (new code) and the OpenSSL and SSLeay licenses (inherited code); the texts in `licenses/LibreSSL.txt` | https://www.libressl.org/ (`ports/libressl`) |
| openssl(1), LibreSSL's command-line tool (the `openssl` package, built from the same release) | 4.3.3 (`ports/libressl/versions.env`) | LibreSSL's, as above | https://www.libressl.org/ (`packages/openssl`) |
| acme.sh, the ACME client (the `acme` package: the script with its dnsapi, deploy and notify hooks; the myos patch, `packages/acme/acme.sh.myos.patch`, under the same license) | 3.1.6 | **GPL-3.0** (`licenses/GPL-3.0.txt`) | https://github.com/acmesh-official/acme.sh |
| curl | 8.11.1 | curl license (MIT-style) | https://curl.se/ |
| Mozilla CA certificate bundle (`/lib/cacert.pem`, from curl.se) | latest at build time | **MPL-2.0** | https://curl.se/docs/caextract.html |
| os-test suite (`/lib/os-test`, when included) | `OSTEST_REV` in `packages/os-test/versions.env` | ISC | https://gitlab.com/sortix/os-test |
| BSD `syslimits.h` (`toolchain/newlib/libgloss/myos/sys/`) | n/a (file copied into this repo) | BSD-3-Clause, Regents of the University of California (notice kept in the file) | FreeBSD |

Portions of this software are copyright © The FreeType Project
(www.freetype.org). All rights reserved. (The FreeType License's credit, for
the `x11-xft` package and the X programs linked with it.)

Files in this repository that carry notices from upstream (kept as required):
`toolchain/newlib/libgloss/myos/{basename.c,dirname.c}` (newlib, Shaun Jackman),
`toolchain/newlib/libgloss/myos/inet.c` (Paul Vixie / ISC),
`toolchain/newlib/libgloss/myos/sys/syslimits.h` (UC Regents),
`ports/termcap/termcap`'s `st` entries (the st authors).

## Optional components (`--features linux_compat`) and build-time tools

| Component | Version | License | Notes |
|-----------|---------|---------|-------|
| musl libc (Linux test binaries and their `libc.so`) | 1.2.5 | MIT | fetched by `linux-compat/build.sh` |
| zlib (linked into `get-alpine`) | see above | zlib | the `ports/zlib` build |
| LLVM compiler-rt builtins (binary128 `long double` helpers, `libtf.a` / mlibc's `libbuiltins.a`) | llvmorg-19.1.7 | Apache-2.0 WITH LLVM-exception | fetched by `linux-compat/build.sh` and `toolchain/mlibc/fetch.sh` (every program linked against mlibc) |
| rustls and ring (the launcher's test web server, `src/test_www.rs`: HTTPS on the host for the boot tests) | rustls 0.23, ring 0.17 (`Cargo.lock`) | rustls: Apache-2.0 OR ISC OR MIT; ring: Apache-2.0 AND ISC | host-only crates of the launcher (already in its build through `ovmf-prebuilt`); not part of any image |
| EDK2 / OVMF firmware (used on the host only to boot QEMU) | via `ovmf-prebuilt` 0.2.9 (MIT OR Apache-2.0) | BSD-2-Clause-Patent | not part of any image |
| U-Boot (`cargo run -- rpi3`: the Raspberry Pi 3's boot loader, `boot/rpi3`) | `UBOOT_VERSION` in `boot/rpi3/versions.env` | **GPL-2.0-or-later** (`licenses/GPL-2.0.txt`) | built on the host from the pinned release tarball into `target/rpi3`, used unmodified; not part of any image |
| The Raspberry Pi 3's device tree (`bcm2710-rpi-3-b.dtb`, from the Raspberry Pi firmware) | `FIRMWARE_TAG` in `boot/rpi3/versions.env` | **GPL-2.0** (the Raspberry Pi Linux kernel's device tree sources) | downloaded on the host by `boot/rpi3/build.sh`, SHA-256 pinned; not part of any image |
| Alpine Linux packages | n/a | per package | downloaded **at run time** by `get-alpine` on the user's machine; nothing from Alpine is in the image |
| Intel Wi-Fi firmware (`iwlwifi-QuZ-a0-hr-b0-77.ucode`, the AX201's; `iwlwifi-cc-a0-77.ucode`, the AX200's) | linux-firmware `a2dcb924` | Intel's redistributable licence (`LICENCE.iwlwifi_firmware` in linux-firmware: binary only, unmodified, no reverse engineering) | downloaded **at run time** by `wifi-firmware fetch` on the user's machine, SHA-256 pinned; not in the image |
| MediaTek MT7601U firmware (`mt7601u.bin`) | linux-firmware `a2dcb924` | Ralink's redistributable licence (`LICENSES/LICENCE.ralink_a_mediatek_company_firmware` in linux-firmware: binary only, unmodified, no reverse engineering) | downloaded **at run time** by `wifi-firmware fetch` on the user's machine, SHA-256 pinned; not in the image |
| MediaTek MT7612U firmware and ROM patch (`mt7662.bin`, `mt7662_rom_patch.bin`: the MT7662E's, which Linux's mt76x2u loads too) | linux-firmware `a2dcb924` | Ralink's redistributable licence (`LICENSES/LICENCE.ralink_a_mediatek_company_firmware` in linux-firmware: binary only, unmodified, no reverse engineering) | downloaded **at run time** by `wifi-firmware fetch` on the user's machine, SHA-256 pinned; not in the image |
| Cypress/Infineon Wi-Fi firmware for the Raspberry Pi's chips (`cyfmac43455-sdio.bin` and `.clm_blob`, the CYW43455's; `cyfmac43430-sdio.bin` and `.clm_blob`, the CYW43438's) | linux-firmware `a2dcb924` | Cypress's redistributable licence (`LICENCE.cypress` in linux-firmware: binary only, unmodified, no reverse engineering) | downloaded **at run time** by `wifi-firmware fetch` on the user's machine, SHA-256 pinned; not in the image |
| The Raspberry Pi boards' Wi-Fi NVRAM files (`brcmfmac43455-sdio.raspberrypi,3-model-b-plus.txt`, `brcmfmac43455-sdio.raspberrypi,4-model-b.txt`, `brcmfmac43430-sdio.raspberrypi,3-model-b.txt`: the chip's board settings as text) | linux-firmware `a2dcb924` | **GPL-2.0-only** (as linux-firmware's `WHENCE` lists them; `licenses/GPL-2.0.txt`) | downloaded **at run time** by `wifi-firmware fetch` on the user's machine, SHA-256 pinned; not in the image, not patched |

## Source code for copyleft components

The following programs in the image are under copyleft licenses
(GPL-2.0-only, GPL-3.0-or-later, GPL-3.0, LGPL-2.1, MPL-2.0): **Git, Lynx,
GNU Make, TinyCC, TinyX (`Xfbdev`), acme.sh, the Mozilla CA bundle, and
`option-ext` in the bottom package, `certifi` (pip's, its CA bundle) in the python package,
GLib, GNU libiconv and proxy-libintl in the glib package, fribidi,
cairo, pango and gdk-pixbuf in the cairo-pango package, ATK and GTK in
the gtk3 package, libsoup, glib-networking and libpsl's Public Suffix
List in the libsoup package, libgpg-error, libgcrypt, libtasn1 and
WebKitGTK in the webkitgtk package.** (The glib, cairo-pango, gtk3, libsoup and webkitgtk packages' programs link those LGPL libraries statically: the programs'
sources and every build script are in this repository too, so they can
be relinked against a modified library.) (Vim's license also allows redistribution. LibreSSL's
inherited OpenSSL and SSLeay licenses ask that binary distributions carry
their notices: `licenses/LibreSSL.txt`, and this product includes software
developed by the OpenSSL Project for use in the OpenSSL Toolkit
(http://www.openssl.org/) and cryptographic software written by Eric Young
(eay@cryptsoft.com).)

The complete corresponding source for each of them is:

1. the pinned upstream source named in the table above (exact revision or
   version and, where available, SHA-256 in `ports/<name>/versions.env` or
   `packages/<name>/versions.env`), plus
2. the myos build scripts and patches in this repository, at the git commit
   from which the image was built (`ports/<name>/`, `packages/<name>/`,
   `scripts/`, `toolchain/`). The TinyX patches (`packages/tinyx/*.myos.patch`,
   the new `kdrive/myos` files among them) are under TinyX's license.

If you received a myos image or ISO from the maintainers and would like these
sources on a physical medium, or cannot download them from the locations above,
open an issue at https://github.com/davide-matasci/myos/issues. For at
least three years after distributing an image, the maintainers will provide a
copy of the corresponding source of those components, for a charge no more
than the cost of physically performing the distribution.

## License texts

`LICENSE-APACHE`, `LICENSE-MIT` and the texts of the copyleft licenses in
[`licenses/`](licenses/) (GPL-2.0, GPL-3.0, LGPL-2.1, MPL-2.0) are copied into
the root of the ISO built by `cargo run -- iso`, next to this file. The
license text of every other component is in its upstream source tree (the
pinned source named above); keep these notices intact when you redistribute an
image.

## Keeping this file correct

When you bump a version in a `versions.env` / `fetch.sh`, or add a new port,
re-check the upstream license (it occasionally changes between releases) and
update the table above in the same change.
