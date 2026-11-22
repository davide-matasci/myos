#!/bin/sh
# Host-prebuild the boot-CI curated os-test binaries (every list
# ci-boot.tests includes) so the guest only runs them.
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
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
. "$ROOT/scripts/myos-c-userspace-lib.sh"

BOOT_LIST="$HERE/overlay/misc/ci-boot.tests"
BASIC_LIST="$HERE/overlay/misc/ci-basic-smoke.tests"
NONBASIC_LIST="$HERE/overlay/misc/ci-nonbasic-100.tests"
EXPANSION_LIST="$HERE/overlay/misc/ci-expansion.tests"
EXPANSION2_LIST="$HERE/overlay/misc/ci-expansion-2.tests"
UDP_LIST="$HERE/overlay/misc/ci-udp.tests"
EMBED="${OSTEST_EMBED:-$ROOT/target/os-test-embed}"
OUT_ROOT="$ROOT/target/os-test-prebuilt"

if [ ! -f "$BOOT_LIST" ]; then
  echo "error: missing $BOOT_LIST" >&2
  exit 1
fi

if [ ! -d "$EMBED/basic" ]; then
  echo "==> os-test embed missing; fetching"
  "$HERE/fetch.sh"
fi

"$ROOT/toolchain/newlib/tool-wrappers.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"
myos_ensure_llvm_bin

# Resolve suite-prefixed vs basic-relative path → "suite/rel" (no .c).
resolve_rel() (
  path="$1"
  suite="${path%%/*}"
  rest="${path#*/}"
  if [ "$suite" != "$path" ] && [ -f "$EMBED/$suite/${rest}.c" ]; then
    echo "$suite/$rest"
  elif [ -f "$EMBED/basic/${path}.c" ]; then
    echo "basic/$path"
  else
    echo "error: cannot resolve test path: $path" >&2
    return 1
  fi
)

# Parse TESTS += from a fragment.
parse_tests() (
  list="$1"
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
    TESTS\ +=*)
      rel="${line#TESTS +=}"
      rel="${rel#"${rel%%[![:space:]]*}"}"
      [ -n "$rel" ] && echo "$rel"
      ;;
    esac
  done < "$list"
)

TESTS=""
NTESTS=0
while IFS= read -r path || [ -n "$path" ]; do
  [ -z "$path" ] && continue
  resolved="$(resolve_rel "$path")" || exit 1
  TESTS="$TESTS $resolved"
  NTESTS=$((NTESTS + 1))
done <<EOF
$(
  parse_tests "$BASIC_LIST"
  parse_tests "$NONBASIC_LIST"
  parse_tests "$EXPANSION_LIST"
  parse_tests "$EXPANSION2_LIST"
  parse_tests "$UDP_LIST"
)
EOF

if [ "$NTESTS" -eq 0 ]; then
  echo "error: no TESTS in boot lists" >&2
  exit 1
fi

link_one() {
  _lo_arch="$1"
  _lo_rel="$2"   # e.g. basic/arpa_inet/htons or limits/CHAR_BIT
  _lo_triple="${_lo_arch}-unknown-myos"
  _lo_prefix="$ROOT/target/newlib-${_lo_arch}"
  _lo_inc="$_lo_prefix/${_lo_triple}/include"
  _lo_lib="$_lo_prefix/${_lo_triple}/lib"
  _lo_src="$EMBED/${_lo_rel}.c"
  _lo_outdir="$OUT_ROOT/${_lo_arch}/$(dirname "$_lo_rel")"
  _lo_out="$OUT_ROOT/${_lo_arch}/${_lo_rel}"
  _lo_obj="$ROOT/target/os-test-prebuilt-obj/${_lo_arch}/${_lo_rel}.o"
  _lo_cc="${_lo_triple}-cc"
  _lo_extra=""

  if [ ! -f "$_lo_src" ]; then
    echo "error: missing source $_lo_src" >&2
    return 1
  fi
  if [ ! -f "$_lo_lib/crt0.o" ] || [ ! -f "$_lo_lib/libc.a" ] || [ ! -f "$_lo_lib/libgloss.a" ]; then
    echo "error: newlib sysroot incomplete for $_lo_arch at $_lo_lib" >&2
    return 1
  fi

  mkdir -p "$_lo_outdir" "$(dirname "$_lo_obj")"
  echo "==> os-test prebuild $_lo_arch $_lo_rel"
  # -I the suite dir so "suite.h" / "malloc.h" / "io.h" resolve like guest cwd.
  "$_lo_cc" -ffreestanding -fPIC -O2 -isystem "$_lo_inc" \
    \
    -D_GNU_SOURCE -D_BSD_SOURCE -D_ALL_SOURCE -D_DEFAULT_SOURCE \
    -c "$_lo_src" -o "$_lo_obj"

  if [ "$_lo_arch" = "aarch64" ] || [ "$_lo_arch" = "riscv64" ]; then
    _lo_tf="$ROOT/target/os-test-prebuilt-obj/${_lo_arch}/${_lo_rel}-trunctfdf2.o"
    mkdir -p "$(dirname "$_lo_tf")"
    "$_lo_cc" -ffreestanding -fPIC -O2 -isystem "$_lo_inc" \
      -c "$ROOT/ports/sbase/trunctfdf2.c" -o "$_lo_tf"
    _lo_extra="$_lo_tf"
  fi

  # -lm for float printf and the math/complex/fenv tests; a static archive,
  # so it adds nothing to the tests that do not use it.
  _lo_libs="-lc -lgloss -lg -lm"

  # Stripped: the symbol tables are most of a static test's size, and the
  # full boot installs and copies all of them (833 tests).
  # shellcheck disable=SC2086
  ld.lld -pie --no-dynamic-linker --strip-all -o "$_lo_out" \
    --entry=_start -z max-page-size=4096 \
    "$_lo_lib/crt0.o" "$_lo_obj" ${_lo_extra:+"$_lo_extra"} \
    -L"$_lo_lib" --start-group $_lo_libs --end-group
}

ARCHES="x86_64"
for a in aarch64 riscv64; do
  if [ -f "$ROOT/target/newlib-${a}/${a}-unknown-myos/lib/crt0.o" ]; then
    ARCHES="$ARCHES $a"
  fi
done

for arch in $ARCHES; do
  for rel in $TESTS; do
    link_one "$arch" "$rel"
  done
  echo "os-test prebuilt $arch: $NTESTS binaries -> $OUT_ROOT/$arch"
done
