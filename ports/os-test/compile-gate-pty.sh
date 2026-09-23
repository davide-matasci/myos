#!/usr/bin/env bash
# Compile gate for the os-test pty suite: compile + link every
# target/os-test-embed/pty/*.c with the exact ports/os-test/
# prebuild-basic-smoke.sh link_one recipe (x86_64). Exit non-zero on any
# failure.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
source "$ROOT/scripts/myos-c-userspace-lib.sh"
EMBED="${OSTEST_EMBED:-$ROOT/target/os-test-embed}"
OUT_ROOT="$ROOT/target/os-test-prebuilt"

"$ROOT/toolchain/newlib/tool-wrappers.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"
myos_ensure_llvm_bin

ARCHES=(x86_64)
for a in aarch64 riscv64; do
  if [[ -f "$ROOT/target/newlib-${a}/${a}-unknown-myos/lib/crt0.o" ]]; then
    ARCHES+=("$a")
  fi
done

total_pass=0
total_fail=0
for arch in "${ARCHES[@]}"; do
  triple="${arch}-unknown-myos"
  prefix="$ROOT/target/newlib-${arch}"
  inc="$prefix/$triple/include"
  lib="$prefix/$triple/lib"
  cc="${triple}-cc"

  if [[ ! -f "$lib/crt0.o" || ! -f "$lib/libc.a" || ! -f "$lib/libgloss.a" ]]; then
    echo "error: newlib sysroot incomplete at $lib" >&2
    exit 1
  fi

  mkdir -p "$OUT_ROOT/$arch/pty" "$ROOT/target/os-test-prebuilt-obj/$arch/pty"

  pass=0
  fail=0
  for src in "$EMBED"/pty/*.c; do
    rel="pty/$(basename "$src" .c)"
    obj="$ROOT/target/os-test-prebuilt-obj/$arch/${rel}.o"
    out="$OUT_ROOT/$arch/${rel}"
    extra=()
    echo "==> pty gate $arch $rel"
    if ! "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
        -D_GNU_SOURCE -D_BSD_SOURCE -D_ALL_SOURCE -D_DEFAULT_SOURCE \
        -c "$src" -o "$obj"; then
      fail=$((fail + 1))
      continue
    fi
    # Same aid objects as ports/os-test/prebuild-basic-smoke.sh link_one.
    if [[ "$arch" == "aarch64" ]]; then
      tf="$ROOT/target/os-test-prebuilt-obj/$arch/${rel}-trunctfdf2.o"
      "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
        -c "$ROOT/ports/sbase/trunctfdf2.c" -o "$tf"
      extra+=("$tf")
    fi
    if [[ "$arch" == "riscv64" ]]; then
      sf="$ROOT/target/os-test-prebuilt-obj/$arch/${rel}-riscv64-softfloat.o"
      "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
        -c "$ROOT/ports/sbase/riscv64-softfloat.c" -o "$sf"
      extra+=("$sf")
    fi
    if ! ld.lld -pie --no-dynamic-linker -o "$out" \
        --entry=_start -z max-page-size=4096 \
        "$lib/crt0.o" "$obj" "${extra[@]+"${extra[@]}"}" \
        -L"$lib" --start-group -lc -lgloss -lg --end-group; then
      fail=$((fail + 1))
      continue
    fi
    pass=$((pass + 1))
  done
  echo "pty compile gate ($arch): $pass passed, $fail failed"
  total_pass=$((total_pass + pass))
  total_fail=$((total_fail + fail))
done

echo "pty compile gate: $total_pass passed, $total_fail failed"
[[ $total_fail -eq 0 ]]