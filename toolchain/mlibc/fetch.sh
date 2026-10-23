#!/usr/bin/env bash
# Fetch mlibc at its pinned tag, and the meson subprojects its build takes
# (frigg, libsmarter, the freestanding C and C++ headers) at the revisions
# its wrap files name, into target/mlibc-src: meson uses a subproject that
# is already a directory and downloads nothing. The compiler-rt builtins
# build.sh compiles into libbuiltins.a go to target/mlibc-compiler-rt, one
# file at a time as ports/curl/build-softfloat-riscv64.sh fetches them.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=toolchain/mlibc/versions.env
source "$HERE/versions.env"
SRC="$ROOT/target/mlibc-src"
CRT="$ROOT/target/mlibc-compiler-rt"

clone_at() {
  local url="$1" rev="$2" dir="$3"
  if [[ -d "$dir/.git" ]] && [[ "$(git -C "$dir" rev-parse HEAD)" == "$rev" ]]; then
    return 0
  fi
  rm -rf "$dir"
  "$ROOT/scripts/git-retry.sh" clone --filter=blob:none --no-checkout "$url" "$dir"
  git -C "$dir" checkout -q "$rev"
}

echo "==> fetch mlibc ($MLIBC_TAG)"
clone_at https://github.com/managarm/mlibc.git "$MLIBC_REV" "$SRC"
clone_at https://github.com/managarm/frigg.git "$FRIGG_REV" "$SRC/subprojects/frigg"
clone_at https://github.com/managarm/libsmarter.git "$LIBSMARTER_REV" "$SRC/subprojects/libsmarter"
clone_at https://github.com/osdev0/freestnd-c-hdrs.git "$FREESTND_C_HDRS_REV" "$SRC/subprojects/freestnd-c-hdrs"
clone_at https://github.com/osdev0/freestnd-cxx-hdrs.git "$FREESTND_CXX_HDRS_REV" "$SRC/subprojects/freestnd-cxx-hdrs"
# What the wrap files lay over the two header trees (their meson.build).
for p in freestnd-c-hdrs freestnd-cxx-hdrs; do
  cp -a "$SRC/subprojects/packagefiles/$p/." "$SRC/subprojects/$p/"
done

# compiler-rt's builtins (build.sh picks each arch's): the IEEE binary128
# (`long double` on aarch64 and riscv64) arithmetic, conversions and
# compares, the double and float ones of riscv64 (soft-float), the
# complex multiplications and the 128-bit integer divisions; the headers
# they include.
CRT_FILES=(
  addtf3.c subtf3.c multf3.c divtf3.c comparetf2.c
  extenddftf2.c extendsftf2.c trunctfdf2.c trunctfsf2.c
  fixtfdi.c fixtfsi.c fixunstfdi.c fixunstfsi.c
  floatditf.c floatsitf.c floatunditf.c floatunsitf.c
  adddf3.c subdf3.c muldf3.c divdf3.c comparedf2.c
  addsf3.c subsf3.c mulsf3.c divsf3.c comparesf2.c
  fixdfsi.c fixdfdi.c fixunsdfsi.c fixunsdfdi.c
  fixsfsi.c fixsfdi.c fixunssfsi.c fixunssfdi.c
  floatsidf.c floatdidf.c floatunsidf.c floatundidf.c
  floatsisf.c floatdisf.c floatunsisf.c floatundisf.c
  truncdfsf2.c extendsfdf2.c
  muldc3.c mulsc3.c multc3.c
  udivmodti4.c udivti3.c umodti3.c divti3.c modti3.c divmodti4.c
  int_lib.h int_types.h int_util.h int_endianness.h int_math.h int_div_impl.inc
  fp_lib.h fp_mode.h fp_add_impl.inc fp_div_impl.inc fp_mul_impl.inc
  fp_extend.h fp_extend_impl.inc fp_trunc.h fp_trunc_impl.inc
  fp_fixint_impl.inc fp_fixuint_impl.inc fp_compare_impl.inc
  int_to_fp.h int_to_fp_impl.inc
)
echo "==> fetch compiler-rt builtins ($COMPILER_RT_TAG)"
mkdir -p "$CRT"
for f in "${CRT_FILES[@]}"; do
  [[ -f "$CRT/$f" ]] && continue
  for url in "https://cdn.jsdelivr.net/gh/llvm/llvm-project@$COMPILER_RT_TAG/compiler-rt/lib/builtins/$f" \
      "https://raw.githubusercontent.com/llvm/llvm-project/$COMPILER_RT_TAG/compiler-rt/lib/builtins/$f"; do
    if curl -fsSL --retry 3 --retry-all-errors --retry-delay 2 -o "$CRT/$f.partial" "$url"; then
      mv "$CRT/$f.partial" "$CRT/$f"
      continue 2
    fi
  done
  echo "compiler-rt fetch failed: $f" >&2
  exit 1
done
