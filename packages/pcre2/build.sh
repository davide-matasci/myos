#!/bin/sh
# Cross-build static libpcre2-8.a (Unicode, no JIT) for the three arches,
# against newlib (target/pcre2-<arch>, for nginx) and against mlibc
# (target/pcre2-mlibc-<arch>, for packages/glib).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=versions.env
. "$HERE/versions.env"
# shellcheck source=scripts/myos-c-userspace-lib.sh
. "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_pcre2_is_current; then
  echo "pcre2 libs up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/newlib/build.sh"
"$ROOT/toolchain/mlibc/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"
myos_ensure_llvm_bin

SRC="$ROOT/target/pcre2-src"

# COMMON_SOURCES of Makefile.am (the .c files) and the generated tables.
PCRE2_SRCS="
  pcre2_auto_possess.c pcre2_chkdint.c pcre2_compile.c pcre2_compile_cgroup.c
  pcre2_compile_class.c pcre2_config.c pcre2_context.c pcre2_convert.c
  pcre2_dfa_match.c pcre2_error.c pcre2_extuni.c pcre2_find_bracket.c
  pcre2_jit_compile.c pcre2_maketables.c pcre2_match.c pcre2_match_data.c
  pcre2_match_next.c pcre2_newline.c pcre2_ord2utf.c pcre2_pattern_info.c
  pcre2_script_run.c pcre2_serialize.c pcre2_string_utils.c pcre2_study.c
  pcre2_substitute.c pcre2_substring.c pcre2_tables.c pcre2_ucd.c
  pcre2_valid_utf.c pcre2_xclass.c pcre2_chartables.c
"

# build_arch ARCH LIBC: target/pcre2-<arch> for newlib, pcre2-mlibc-<arch>
# for mlibc.
build_arch() (
  arch="$1" libc="$2"
  triple="${arch}-unknown-myos"
  cc="${triple}-cc" ar="${triple}-ar"
  out="$ROOT/target/pcre2-${arch}"
  gen="$out/gen"
  # The compiler's flags are the positional parameters: the libc's first.
  set -- \
    -ffreestanding -fPIC -nostdinc -isystem "$(clang -print-resource-dir)/include" \
    -isystem "$ROOT/target/newlib-${arch}/${triple}/include"
  if [ "$libc" = mlibc ]; then
    cc=clang
    out="$ROOT/target/pcre2-mlibc-${arch}" gen="$out/gen"
    # shellcheck disable=SC2046
    set -- $(myos_mlibc_cflags "$arch")
  fi
  set -- \
    "$@" -O2 -g0 -std=gnu99 \
    -I"$gen" -I"$SRC/src" \
    -DHAVE_CONFIG_H -DPCRE2_CODE_UNIT_WIDTH=8 -DPCRE2_STATIC \
    -DSUPPORT_PCRE2_8 -DSUPPORT_UNICODE \
    -DHAVE_STDINT_H -DHAVE_INTTYPES_H -DHAVE_LIMITS_H -DHAVE_STRING_H \
    -DHAVE_STDLIB_H -DHAVE_ASSERT_H
  objs="" nobjs=0

  echo "pcre2: building $arch ($libc)"
  rm -rf "$out"
  mkdir -p "$gen" "$out/obj" "$out/lib" "$out/include"
  # What configure would generate: the generic config and header, the
  # default character tables.
  cp "$SRC/src/config.h.generic" "$gen/config.h"
  cp "$SRC/src/pcre2.h.generic" "$gen/pcre2.h"
  cp "$SRC/src/pcre2_chartables.c.dist" "$gen/pcre2_chartables.c"

  for name in $PCRE2_SRCS; do
    if [ -f "$gen/$name" ]; then src="$gen/$name"; else src="$SRC/src/$name"; fi
    o="$out/obj/${name%.c}.o"
    "$cc" "$@" -c "$src" -o "$o"
    objs="$objs $o" nobjs=$((nobjs + 1))
  done
  # shellcheck disable=SC2086
  "$ar" rcs "$out/lib/libpcre2-8.a" $objs
  cp "$gen/pcre2.h" "$out/include/"
  echo "pcre2 $arch ($libc): $nobjs objs -> $out/lib/libpcre2-8.a"
)

for arch in x86_64 aarch64 riscv64; do
  build_arch "$arch" newlib
  build_arch "$arch" mlibc
done
echo "$(myos_pcre2_version_hash)" >"$MYOS_PCRE2_VERSION"
echo "pcre2 build ok"
