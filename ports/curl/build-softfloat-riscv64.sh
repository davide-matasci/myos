#!/usr/bin/env bash
# Build soft-float compiler-rt helpers for riscv64 (no F/D extension).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="$ROOT/target/libsoftfloat-riscv64.a"
SRC="$ROOT/target/compiler-rt-sf"
OBJ="$SRC/obj"
BASE=https://raw.githubusercontent.com/llvm/llvm-project/llvmorg-19.1.7/compiler-rt/lib/builtins
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
  fixdfsi.c fixdfdi.c fixunsdfsi.c fixunsdfdi.c
  floatsidf.c floatdidf.c floatunsidf.c floatundidf.c
  truncdfsf2.c extendsfdf2.c ashldi3.c ashrdi3.c lshrdi3.c
  int_lib.h int_types.h int_util.h int_endianness.h int_math.h
  fp_lib.h fp_mode.h fp_add_impl.inc fp_div_impl.inc fp_mul_impl.inc
  fp_extend_impl.inc fp_trunc_impl.inc fp_extend.h fp_trunc.h
  int_to_fp_impl.inc fp_fixint_impl.inc fp_fixuint_impl.inc fp_compare_impl.inc
)
for f in "${FILES[@]}"; do
  [[ -f "$SRC/$f" ]] || curl -fsSL -o "$SRC/$f" "$BASE/$f"
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
