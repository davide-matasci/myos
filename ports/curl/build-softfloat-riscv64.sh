#!/usr/bin/env bash
# Build soft-float compiler-rt helpers for riscv64 (no F/D extension).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="$ROOT/target/libsoftfloat-riscv64.a"
SRC="$ROOT/target/compiler-rt-sf"
OBJ="$SRC/obj"
# Prefer jsDelivr (same llvmorg tag) — raw.githubusercontent.com 429s on burst ISO fetches.
BASE_JSDELIVR=https://cdn.jsdelivr.net/gh/llvm/llvm-project@llvmorg-19.1.7/compiler-rt/lib/builtins
BASE_GITHUB=https://raw.githubusercontent.com/llvm/llvm-project/llvmorg-19.1.7/compiler-rt/lib/builtins
export PATH="$ROOT/target/newlib-bin:$PATH"
cc=riscv64-unknown-myos-cc
inc="$ROOT/target/newlib-riscv64/riscv64-unknown-myos/include"

if [[ -f "$OUT" && -f "$SRC/.stamp" ]]; then
  echo "softfloat riscv64 up to date"
  exit 0
fi

mkdir -p "$SRC" "$OBJ"
FILES=(
  adddf3.c subdf3.c muldf3.c divdf3.c comparedf2.c comparesf2.c
  addsf3.c subsf3.c mulsf3.c divsf3.c
  fixdfsi.c fixdfdi.c fixunsdfsi.c fixunsdfdi.c
  fixsfsi.c fixunssfsi.c
  floatsidf.c floatdidf.c floatunsidf.c floatundidf.c
  floatsisf.c floatunsisf.c
  truncdfsf2.c extendsfdf2.c ashldi3.c ashrdi3.c lshrdi3.c
  int_lib.h int_types.h int_util.h int_endianness.h int_math.h
  fp_lib.h fp_mode.h fp_add_impl.inc fp_div_impl.inc fp_mul_impl.inc
  fp_extend_impl.inc fp_trunc_impl.inc fp_extend.h fp_trunc.h
  int_to_fp_impl.inc fp_fixint_impl.inc fp_fixuint_impl.inc fp_compare_impl.inc
)
# Prefer jsDelivr; fall back to GitHub raw with retries (ISO hit HTTP 429 on raw bursts).
fetch_one() {
  local out="$1"; shift
  local url attempt delay
  for url in "$@"; do
    for attempt in 1 2 3 4 5; do
      if curl -fsSL --retry 3 --retry-all-errors --retry-delay 2 \
           -o "${out}.partial" "$url"; then
        mv "${out}.partial" "$out"
        return 0
      fi
      delay=$((attempt * 2))
      echo "softfloat fetch retry ${attempt}/5 (${url##*/}) after ${delay}s" >&2
      sleep "$delay"
    done
  done
  echo "softfloat fetch failed: $(basename "$out")" >&2
  return 1
}
for f in "${FILES[@]}"; do
  if [[ ! -f "$SRC/$f" ]]; then
    fetch_one "$SRC/$f" "$BASE_JSDELIVR/$f" "$BASE_GITHUB/$f"
  fi
done
cat > "$SRC/fe_stubs.c" <<'C'
int __fe_getround(void) { return 0; }
int __fe_raise_inexact(void) { return 0; }
C

for f in "$SRC"/*.c; do
  bn=$(basename "$f" .c)
  "$cc" -ffreestanding -fPIC -O2 -I"$SRC" -isystem "$inc" -c "$f" -o "$OBJ/$bn.o"
done
ar rcs "$OUT" "$OBJ"/*.o
echo ok >"$SRC/.stamp"
echo "softfloat -> $OUT"
