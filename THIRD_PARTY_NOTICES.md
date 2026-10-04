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
| Limine bootloader (BIOS/UEFI binaries) | 12.6.1 | BSD-2-Clause | https://github.com/limine-bootloader/limine (`src/limine_image.rs`) |
| newlib + libgloss (C library; `toolchain/newlib/`) | 4.4.0 | Mostly BSD-style permissive licenses; see `COPYING.NEWLIB` / `COPYING.LIBGLOSS` in the upstream source | https://sourceware.org/git/newlib-cygwin.git |
| Rust `std` and `core` (patched; `toolchain/std/`), linked into Rust userspace programs | pinned nightly (`rust-toolchain.toml`) | MIT OR Apache-2.0 | https://github.com/rust-lang/rust |
| Rust crates in the kernel / `user/netd` | see `Cargo.toml` | `limine`: MIT OR Apache-2.0; `spin`: MIT; `x86_64`, `linked_list_allocator`, `pic8259`: MIT/Apache-2.0; `smoltcp`: 0BSD; `fdt` (device tree parser, used unmodified): **MPL-2.0** (`licenses/MPL-2.0.txt`) | https://crates.io |
| Rust crates in the `xhci` module (`modules/xhci/Cargo.toml`) | `xhci` 0.9.2 | `xhci` (register, TRB and context layouts, used unmodified) and its dependencies `accessor`, `bit_field`, `num-derive`, `num-traits`, `paste`: MIT OR Apache-2.0 | https://github.com/rust-osdev/xhci |
| sbase | `SBASE_REV` in `ports/sbase/versions.env` | MIT | https://git.suckless.org/sbase |
| ubase | `UBASE_REV` in `ports/ubase/versions.env` | MIT | https://github.com/michaelforney/ubase |
| uutils coreutils (+ its Rust dependencies) | 0.10.0 (see `ports/coreutils/README.md`) | MIT (dependencies: MIT / Apache-2.0 and similar permissive licenses) | https://github.com/uutils/coreutils |
| ripgrep (+ its Rust dependencies) | 15.2.0 | Unlicense OR MIT | https://github.com/BurntSushi/ripgrep |
| PCRE2 (for ripgrep) | 10.45 | BSD-3-Clause | https://github.com/PCRE2Project/pcre2 |
| oksh (`/sh`) | 7.9 | Public domain (core) + BSD/ISC (portability files) | https://github.com/ibara/oksh |
| Dropbear SSH | 2026.94 | MIT-style (plus BSD / public-domain parts; see upstream `LICENSE`) | https://matt.ucc.asn.au/dropbear/ |
| TinyCC (`tcc`) and `libtcc1` | `TCC_REV` in `ports/tcc/versions.env` | **LGPL-2.1** | https://github.com/TinyCC/tinycc |
| Vim | 9.2.0 | Vim license (GPL-compatible, "charityware") | https://github.com/vim/vim |
| ncurses | 6.5 | X11/MIT-style | https://ftp.gnu.org/gnu/ncurses/ |
| zlib | 1.3.1 | zlib | https://github.com/madler/zlib |
| Git | 2.55.0 | **GPL-2.0-only** (some parts LGPL-2.1+) | https://github.com/git/git |
| Lynx | 2.9.3 | **GPL-2.0-only** | https://invisible-island.net/lynx/ |
| GNU Make | 4.4.1 | **GPL-3.0-or-later** | https://ftp.gnu.org/gnu/make/ |
| Lua | 5.4.7 | MIT | https://www.lua.org/ |
| X client libraries (package `x11-libs`): xorgproto, xtrans, libXau, xcb-proto, libxcb, libX11 | see `packages/x11-libs/versions.env` | MIT / X11-style (X.Org, The Open Group and others; each upstream `COPYING`) | https://www.x.org/releases/individual/ |
| Mbed TLS (TLS for `curl`, `lynx`, `user/tls`) | 3.6.2 | Apache-2.0 OR GPL-2.0-or-later (myos uses it under Apache-2.0) | https://github.com/Mbed-TLS/mbedtls |
| curl | 8.11.1 | curl license (MIT-style) | https://curl.se/ |
| Mozilla CA certificate bundle (`/lib/cacert.pem`, from curl.se) | latest at build time | **MPL-2.0** | https://curl.se/docs/caextract.html |
| os-test suite (`/lib/os-test`, when included) | `OSTEST_REV` in `packages/os-test/versions.env` | ISC | https://gitlab.com/sortix/os-test |
| BSD `syslimits.h` (`toolchain/newlib/libgloss/myos/sys/`) | n/a (file copied into this repo) | BSD-3-Clause, Regents of the University of California (notice kept in the file) | FreeBSD |
| PCRE2 headers (`ports/ripgrep/pcre2-headers/`) | 10.46 | BSD-3-Clause, University of Cambridge (notice kept in the file) | https://github.com/PCRE2Project/pcre2 |

Files in this repository that carry notices from upstream (kept as required):
`toolchain/newlib/libgloss/myos/{basename.c,dirname.c}` (newlib, Shaun Jackman),
`toolchain/newlib/libgloss/myos/inet.c` (Paul Vixie / ISC),
`toolchain/newlib/libgloss/myos/sys/syslimits.h` (UC Regents),
`ports/ripgrep/pcre2-headers/pcre2.h` (University of Cambridge).

## Optional components (`--features linux_compat`) and build-time tools

| Component | Version | License | Notes |
|-----------|---------|---------|-------|
| musl libc (Linux test binaries and their `libc.so`) | 1.2.5 | MIT | fetched by `linux-compat/build.sh` |
| zlib (linked into `get-alpine`) | see above | zlib | the `ports/zlib` build |
| LLVM compiler-rt builtins (soft-float helpers, riscv64 / `libtf.a`) | llvmorg-19.1.7 | Apache-2.0 WITH LLVM-exception | fetched by `ports/curl/build-softfloat-riscv64.sh` and `linux-compat/build.sh` |
| EDK2 / OVMF firmware (used on the host only to boot QEMU) | via `ovmf-prebuilt` 0.2.9 (MIT OR Apache-2.0) | BSD-2-Clause-Patent | not part of any image |
| Alpine Linux packages | n/a | per package | downloaded **at run time** by `get-alpine` on the user's machine; nothing from Alpine is in the image |

## Source code for copyleft components

The following programs in the image are under copyleft licenses
(GPL-2.0-only, GPL-3.0-or-later, LGPL-2.1, MPL-2.0): **Git, Lynx, GNU Make,
TinyCC, and the Mozilla CA bundle.** (Vim's and Mbed TLS's licenses also allow
redistribution; Mbed TLS is used here under its Apache-2.0 option.)

The complete corresponding source for each of them is:

1. the pinned upstream source named in the table above (exact revision or
   version and, where available, SHA-256 in `ports/<name>/versions.env`), plus
2. the myos build scripts and patches in this repository, at the git commit
   from which the image was built (`ports/<name>/`, `scripts/`,
   `toolchain/`).

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
