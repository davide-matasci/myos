#!/usr/bin/env bash
# Host-prebuild boot-CI curated os-test binaries (basic smoke + ~100 non-basic)
# so the guest only runs them.
#
# Why: on x86 TCG (esp. GHA), each guest `tcc … -lm` of an os-test source is
# ~90–120s vs ~1–2s for heap's tiny tcc. Compiling on the host against the
# same newlib/libgloss sysroot keeps the smoke honest (real ELF exec + libc)
# without guest compile cost.
#
# Output: target/os-test-prebuilt/<arch>/<suite>/<path>  (no .c suffix)
# Packed by initramfs.rs into /lib/os-test/prebuilt/…
#
# TESTLIST paths:
#   pwd/setpwent       → basic/pwd/setpwent  (legacy basic-relative)
#   limits/CHAR_BIT    → limits/CHAR_BIT     (suite-prefixed)
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

BOOT_LIST="$HERE/overlay/misc/ci-boot.tests"
BASIC_LIST="$HERE/overlay/misc/ci-basic-smoke.tests"
NONBASIC_LIST="$HERE/overlay/misc/ci-nonbasic-100.tests"
EXTRA_LIST="$HERE/overlay/misc/ci-nonbasic-extra.tests"
EMBED="${OSTEST_EMBED:-$ROOT/target/os-test-embed}"
OUT_ROOT="$ROOT/target/os-test-prebuilt"

if [[ ! -f "$BOOT_LIST" ]]; then
  echo "error: missing $BOOT_LIST" >&2
  exit 1
fi

if [[ ! -d "$EMBED/basic" ]]; then
  echo "==> os-test embed missing; fetching"
  "$HERE/fetch.sh"
fi

"$ROOT/toolchain/newlib/tool-wrappers.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"
myos_ensure_llvm_bin

# Resolve suite-prefixed vs basic-relative path → "suite/rel" (no .c).
resolve_rel() {
  local path="$1"
  local suite rest
  suite="${path%%/*}"
  rest="${path#*/}"
  if [[ "$suite" != "$path" && -f "$EMBED/$suite/${rest}.c" ]]; then
    echo "$suite/$rest"
  elif [[ -f "$EMBED/basic/${path}.c" ]]; then
    echo "basic/$path"
  else
    echo "error: cannot resolve test path: $path" >&2
    return 1
  fi
}

# Parse TESTS += from a fragment (bash 3.2-safe; no mapfile).
parse_tests() {
  local list="$1"
  while IFS= read -r line || [[ -n "$line" ]]; do
    case "$line" in
    TESTS\ +=*)
      rel="${line#TESTS +=}"
      rel="${rel#"${rel%%[![:space:]]*}"}"
      [[ -n "$rel" ]] && echo "$rel"
      ;;
    esac
  done < "$list"
}

TESTS=()
while IFS= read -r path || [[ -n "$path" ]]; do
  [[ -z "$path" ]] && continue
  resolved="$(resolve_rel "$path")" || exit 1
  TESTS+=("$resolved")
done < <({
  parse_tests "$BASIC_LIST"
  parse_tests "$NONBASIC_LIST"
  parse_tests "$EXTRA_LIST"
})

if [[ ${#TESTS[@]} -eq 0 ]]; then
  echo "error: no TESTS in boot lists" >&2
  exit 1
fi

link_one() {
  local arch="$1"
  local rel="$2"   # e.g. basic/arpa_inet/htons or limits/CHAR_BIT
  local triple="${arch}-unknown-myos"
  local prefix="$ROOT/target/newlib-${arch}"
  local inc="$prefix/${triple}/include"
  local lib="$prefix/${triple}/lib"
  local src="$EMBED/${rel}.c"
  local outdir="$OUT_ROOT/${arch}/$(dirname "$rel")"
  local out="$OUT_ROOT/${arch}/${rel}"
  local obj="$ROOT/target/os-test-prebuilt-obj/${arch}/${rel}.o"
  local cc="${triple}-cc"
  local extra=()
  local suite="${rel%%/*}"

  if [[ ! -f "$src" ]]; then
    echo "error: missing source $src" >&2
    return 1
  fi
  if [[ ! -f "$lib/crt0.o" || ! -f "$lib/libc.a" || ! -f "$lib/libgloss.a" ]]; then
    echo "error: newlib sysroot incomplete for $arch at $lib" >&2
    return 1
  fi

  mkdir -p "$outdir" "$(dirname "$obj")"
  echo "==> os-test prebuild $arch $rel"
  # -I the suite dir so "suite.h" / "malloc.h" / "io.h" resolve like guest cwd.
  "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
    \
    -D_GNU_SOURCE -D_BSD_SOURCE -D_ALL_SOURCE -D_DEFAULT_SOURCE \
    -c "$src" -o "$obj"

  if [[ "$arch" == "aarch64" ]]; then
    local tf="$ROOT/target/os-test-prebuilt-obj/${arch}/${rel}-trunctfdf2.o"
    mkdir -p "$(dirname "$tf")"
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
      -c "$ROOT/ports/sbase/trunctfdf2.c" -o "$tf"
    extra+=("$tf")
  fi
  if [[ "$arch" == "riscv64" ]]; then
    local sf="$ROOT/target/os-test-prebuilt-obj/${arch}/${rel}-riscv64-softfloat.o"
    mkdir -p "$(dirname "$sf")"
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
      -c "$ROOT/ports/sbase/riscv64-softfloat.c" -o "$sf"
    extra+=("$sf")
  fi

  # Link -lm for stdio float printf and any suite that needs it.
  local libs=(-lc -lgloss -lg)
  case "$suite" in
  stdio|signal|udp|io|process|malloc|paths|limits)
    libs+=(-lm)
    ;;
  esac

  ld.lld -pie --no-dynamic-linker -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "$obj" "${extra[@]+"${extra[@]}"}" \
    -L"$lib" --start-group "${libs[@]}" --end-group
}

ARCHES=(x86_64)
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
