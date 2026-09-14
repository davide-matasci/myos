#!/usr/bin/env bash
# Host-prebuild the boot-CI basic smoke binaries so the guest only runs them.
#
# Why: on x86 TCG (esp. GHA), each guest `tcc … -lm` of an os-test source is
# ~90–120s vs ~1–2s for heap's tiny tcc. 22 smoke tests cannot fit the 600s
# QEMU budget. Compiling on the host against the same newlib/libgloss sysroot
# (same path as scripts/build-c-hello.sh) keeps the smoke honest (real ELF
# exec + libc) without guest compile cost.
#
# Output: target/os-test-prebuilt/<arch>/basic/<path>  (no .c suffix)
# Packed by initramfs.rs into /lib/os-test/prebuilt/…
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

LIST="$HERE/overlay/misc/ci-basic-smoke.tests"
EMBED="${OSTEST_EMBED:-$ROOT/target/os-test-embed}"
OUT_ROOT="$ROOT/target/os-test-prebuilt"

if [[ ! -f "$LIST" ]]; then
  echo "error: missing $LIST" >&2
  exit 1
fi

if [[ ! -d "$EMBED/basic" ]]; then
  echo "==> os-test embed missing; fetching"
  "$HERE/fetch.sh"
fi

"$ROOT/toolchain/newlib/tool-wrappers.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"
myos_ensure_llvm_bin

# Parse TESTS += paths from the smoke list (bash 3.2-safe; no mapfile).
TESTS=()
while IFS= read -r line || [[ -n "$line" ]]; do
  case "$line" in
  TESTS\ +=*)
    rel="${line#TESTS +=}"
    rel="${rel#"${rel%%[![:space:]]*}"}"
    [[ -n "$rel" ]] && TESTS+=("$rel")
    ;;
  esac
done < "$LIST"
if [[ ${#TESTS[@]} -eq 0 ]]; then
  echo "error: no TESTS in $LIST" >&2
  exit 1
fi

link_one() {
  local arch="$1"
  local rel="$2"   # e.g. arpa_inet/htons
  local triple="${arch}-unknown-myos"
  local prefix="$ROOT/target/newlib-${arch}"
  local inc="$prefix/${triple}/include"
  local lib="$prefix/${triple}/lib"
  local src="$EMBED/basic/${rel}.c"
  local outdir="$OUT_ROOT/${arch}/basic/$(dirname "$rel")"
  local out="$OUT_ROOT/${arch}/basic/${rel}"
  local obj="$ROOT/target/os-test-prebuilt-obj/${arch}/${rel}.o"
  local cc="${triple}-cc"
  local extra=()

  if [[ ! -f "$src" ]]; then
    echo "error: missing source $src" >&2
    return 1
  fi
  if [[ ! -f "$lib/crt0.o" || ! -f "$lib/libc.a" || ! -f "$lib/libgloss.a" ]]; then
    echo "error: newlib sysroot incomplete for $arch at $lib" >&2
    return 1
  fi

  mkdir -p "$outdir" "$(dirname "$obj")"
  echo "==> os-test prebuild $arch basic/$rel"
  # Match guest Makefile CFLAGS; freestanding PIC like other myos C ELFs.
  "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
    -D_GNU_SOURCE -D_BSD_SOURCE -D_ALL_SOURCE -D_DEFAULT_SOURCE \
    -c "$src" -o "$obj"

  if [[ "$arch" == "aarch64" ]]; then
    local tf="$ROOT/target/os-test-prebuilt-obj/${arch}/${rel}-trunctfdf2.o"
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
      -c "$ROOT/ports/sbase/trunctfdf2.c" -o "$tf"
    extra+=("$tf")
  fi
  # riscv64 softfloat provides TF/DF helpers + libgcc-style DF ops (dtoa).
  if [[ "$arch" == "riscv64" ]]; then
    local sf="$ROOT/target/os-test-prebuilt-obj/${arch}/${rel}-riscv64-softfloat.o"
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
      -c "$ROOT/ports/sbase/riscv64-softfloat.c" -o "$sf"
    extra+=("$sf")
  fi

  # Smoke list avoids math/; no -lm (matches honest link for these tests).
  ld.lld -pie --no-dynamic-linker -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "$obj" "${extra[@]+"${extra[@]}"}" \
    -L"$lib" --start-group -lc -lgloss -lg --end-group
}

ARCHES=(x86_64)
# Build all arches when their sysroots exist (CI/full builds).
for a in aarch64 riscv64; do
  if [[ -f "$ROOT/target/newlib-${a}/${a}-unknown-myos/lib/crt0.o" ]]; then
    ARCHES+=("$a")
  fi
done

for arch in "${ARCHES[@]}"; do
  for rel in "${TESTS[@]}"; do
    link_one "$arch" "$rel"
  done
  echo "os-test prebuilt $arch: ${#TESTS[@]} binaries -> $OUT_ROOT/$arch"
done
