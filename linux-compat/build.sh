#!/usr/bin/env bash
# Build the optional Linux compatibility layer's userspace pieces, per arch
# (x86_64, aarch64, riscv64):
#   target/linux-launcher-<arch>-unknown-none  the `linux` launcher (myos newlib,
#                                              build-launcher.sh: in every image)
#   target/linux-smoke-<arch>-linux-musl       a Linux static-PIE test (musl)
#   target/linux-compat/<arch>/ld-musl-<arch>.so.1  musl's libc.so / dynamic linker
#   target/linux-compat/<arch>/{linux-dyn,libsmoke.so,libsmoke2.so}
#                                              a dynamically linked test
# musl is built from its release tarball with clang for each target; on
# aarch64/riscv64 its libc.so links compiler-rt's quad-float builtins (fetched
# per file, like ports/curl/build-softfloat-riscv64.sh). Only needed for
# `cargo build --features linux_compat`; see docs/linux-compat.md.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

MUSL_VERSION=1.2.5
MUSL_SHA256=a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4
ARCHES=(x86_64 aarch64 riscv64)

# Stamped like a port (scripts/ci-registry.sh caches the outputs under
# `linux-compat`); the launcher has its own script and is always rebuilt.
if myos_linux_compat_is_current; then
  echo "linux-compat outputs up to date"
  "$ROOT/linux-compat/build-launcher.sh"
  exit 0
fi

myos_ensure_llvm_bin
# The CI image has clang + lld but not llvm's binutils: fall back like the ports.
AR_BIN="$(command -v llvm-ar 2>/dev/null || echo ar)"
RANLIB_BIN="$(command -v llvm-ranlib 2>/dev/null || echo ranlib)"
"$ROOT/toolchain/newlib/build.sh"
# The cross-compiler wrappers (target/newlib-bin) are not in the CI build
# artifacts, which carry newlib itself; they are cheap to (re)write.
"$ROOT/toolchain/newlib/tool-wrappers.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

# compiler-rt builtins for libc.so on aarch64/riscv64 (128-bit long double).
CRT_TAG=llvmorg-19.1.7
CRT_BASE_JSDELIVR="https://cdn.jsdelivr.net/gh/llvm/llvm-project@$CRT_TAG/compiler-rt/lib/builtins"
CRT_BASE_GITHUB="https://raw.githubusercontent.com/llvm/llvm-project/$CRT_TAG/compiler-rt/lib/builtins"
CRT_SRC="$ROOT/target/linux-compat/compiler-rt-$CRT_TAG"
CRT_C=(
  addtf3.c subtf3.c multf3.c divtf3.c comparetf2.c
  extenddftf2.c extendsftf2.c trunctfdf2.c trunctfsf2.c
  fixtfdi.c fixtfsi.c fixunstfsi.c floatditf.c floatsitf.c floatunsitf.c
  muldc3.c mulsc3.c multc3.c
)
CRT_H=(
  int_lib.h int_types.h int_util.h int_endianness.h int_math.h
  fp_lib.h fp_mode.h fp_add_impl.inc fp_div_impl.inc fp_mul_impl.inc
  fp_extend.h fp_extend_impl.inc fp_trunc.h fp_trunc_impl.inc
  fp_fixint_impl.inc fp_fixuint_impl.inc int_to_fp_impl.inc fp_compare_impl.inc
)
fetch_crt() {
  local f="$1" url
  [[ -f "$CRT_SRC/$f" ]] && return 0
  for url in "$CRT_BASE_JSDELIVR/$f" "$CRT_BASE_GITHUB/$f"; do
    if curl -fsSL --retry 3 --retry-all-errors --retry-delay 2 -o "$CRT_SRC/$f.partial" "$url"; then
      mv "$CRT_SRC/$f.partial" "$CRT_SRC/$f"
      return 0
    fi
  done
  echo "compiler-rt fetch failed: $f" >&2
  return 1
}

# libtf.a: the builtins above for one arch, PIC.
build_libtf() {
  local arch="$1" prefix="$2" out="$3"
  mkdir -p "$CRT_SRC"
  for f in "${CRT_C[@]}" "${CRT_H[@]}"; do fetch_crt "$f"; done
  cat >"$CRT_SRC/fe_stubs.c" <<'C'
/* Round to nearest, no exception flags: what musl's soft long double needs. */
int __fe_getround(void) { return 0; }
int __fe_raise_inexact(void) { return 0; }
C
  local obj="$ROOT/target/linux-compat/crt-obj-$arch"
  rm -rf "$obj"
  mkdir -p "$obj"
  for f in "${CRT_C[@]}" fe_stubs.c; do
    clang --target="$arch-linux-musl" -O2 -fPIC -nostdinc \
      -isystem "$(clang -print-resource-dir)/include" -isystem "$prefix/include" \
      -c "$CRT_SRC/$f" -o "$obj/${f%.c}.o"
  done
  "$AR_BIN" rcs "$out" "$obj"/*.o
}

tarball="$ROOT/target/linux-compat/musl-$MUSL_VERSION.tar.gz"
for arch in "${ARCHES[@]}"; do
  cc=(clang --target="$arch-linux-musl")
  prefix="$ROOT/target/linux-compat/musl-$MUSL_VERSION-$arch"

  # musl (static and shared) for the Linux-side test binaries.
  if [[ ! -f "$prefix/lib/libc.so" ]]; then
    mkdir -p "$ROOT/target/linux-compat"
    if [[ ! -f "$tarball" ]]; then
      curl -sSfL --retry 3 -o "$tarball.tmp" "https://musl.libc.org/releases/musl-$MUSL_VERSION.tar.gz"
      mv "$tarball.tmp" "$tarball"
    fi
    echo "$MUSL_SHA256  $tarball" | sha256sum -c -
    # The headers the builtins need come from the static install.
    libcc=()
    if [[ "$arch" != x86_64 ]]; then
      if [[ ! -f "$prefix/include/stdint.h" ]]; then
        hdrs="$ROOT/target/linux-compat/hdr-$arch"
        rm -rf "$hdrs"
        mkdir -p "$hdrs"
        tar xzf "$tarball" -C "$hdrs"
        make -C "$hdrs/musl-$MUSL_VERSION" ARCH="$arch" prefix="$prefix" install-headers >/dev/null
        rm -rf "$hdrs"
      fi
      build_libtf "$arch" "$prefix" "$ROOT/target/linux-compat/libtf-$arch.a"
      libcc=(LIBCC="$ROOT/target/linux-compat/libtf-$arch.a")
    fi
    work="$ROOT/target/linux-compat/build-$arch"
    rm -rf "$work"
    mkdir -p "$work"
    tar xzf "$tarball" -C "$work"
    (
      cd "$work/musl-$MUSL_VERSION"
      CC="${cc[*]}" AR="$AR_BIN" RANLIB="$RANLIB_BIN" LDFLAGS="-fuse-ld=lld" \
        ./configure --target="$arch-linux-musl" --prefix="$prefix" >/dev/null
      if ! make -j"$(nproc)" "${libcc[@]}" >make.log 2>&1; then
        tail -40 make.log >&2
        exit 1
      fi
      make install >/dev/null
    )
    rm -rf "$work"
  fi

  echo "==> linux-smoke ($arch Linux, musl static-PIE)"
  "${cc[@]}" -static-pie -fPIE -O2 -nostdinc -nostdlib -fuse-ld=lld \
    -isystem "$prefix/include" -isystem "$(clang -print-resource-dir)/include" \
    -o "$ROOT/target/linux-smoke-$arch-linux-musl" \
    "$prefix/lib/rcrt1.o" "$prefix/lib/crti.o" \
    "$ROOT/linux-compat/tests/linux-smoke.c" \
    "$prefix/lib/libc.a" "$prefix/lib/crtn.o"

  echo "==> linux-dyn ($arch Linux, musl dynamic)"
  out="$ROOT/target/linux-compat/$arch"
  mkdir -p "$out"
  ldso="ld-musl-$arch.so.1"
  dyncc=("${cc[@]}" -O2 -nostdinc -nostdlib -fuse-ld=lld
    -isystem "$prefix/include" -isystem "$(clang -print-resource-dir)/include")
  cp "$prefix/lib/libc.so" "$out/$ldso"
  for lib in libsmoke libsmoke2; do
    "${dyncc[@]}" -fPIC -shared -Wl,-soname,"$lib.so" -o "$out/$lib.so" \
      "$ROOT/linux-compat/tests/$lib.c" -L"$prefix/lib" -lc
  done
  "${dyncc[@]}" -fPIE -pie -Wl,--dynamic-linker="/lib/$ldso" -o "$out/linux-dyn" \
    "$prefix/lib/Scrt1.o" "$prefix/lib/crti.o" "$ROOT/linux-compat/tests/linux-dyn.c" \
    "$out/libsmoke.so" -L"$prefix/lib" -lc "$prefix/lib/crtn.o"

done
# The launcher ships in every image; its own script builds it for the 3 arches.
"$ROOT/linux-compat/build-launcher.sh"
myos_linux_compat_version_hash > "$MYOS_LINUX_COMPAT_VERSION"
echo "linux-compat -> target/linux-launcher-*-unknown-none, target/linux-smoke-*-linux-musl, target/linux-compat/<arch>/"
