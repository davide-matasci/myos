#!/usr/bin/env bash
# Cross-build mlibc for x86_64, aarch64 and riscv64 with clang: libc.a, the
# crt objects and the headers go to target/mlibc-<arch> (lib/, include/),
# the sysroot a program is compiled and linked against (docs/mlibc.md).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_mlibc_is_current; then
  echo "mlibc up to date"
  exit 0
fi

for tool in meson ninja; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "error: $tool not found: apt install meson ninja-build (README.md)" >&2
    exit 1
  fi
done
myos_ensure_llvm_bin
AR_BIN="$(command -v llvm-ar 2>/dev/null || echo ar)"
STRIP_BIN="$(command -v llvm-strip 2>/dev/null || echo strip)"

"$HERE/fetch.sh"
"$HERE/patch.sh"
SRC="$ROOT/target/mlibc-src"
CRT="$ROOT/target/mlibc-compiler-rt"

# mlibc is C++23: clang 19 or later (the CI image's; a host whose clang is
# older may have a versioned one beside it).
CLANG=""
for c in clang clang-21 clang-20 clang-19; do
  if command -v "$c" >/dev/null 2>&1 && [[ "$("$c" -dumpversion | cut -d. -f1)" -ge 19 ]]; then
    CLANG="$c"
    break
  fi
done
if [[ -z "$CLANG" ]]; then
  echo "error: mlibc needs clang 19 or later (C++23); none found" >&2
  exit 1
fi
CLANGXX="${CLANG/clang/clang++}"

build_one() {
  local arch="$1"
  local elf="${arch}-unknown-none"
  local build="$ROOT/target/mlibc-build-${arch}"
  local prefix="$ROOT/target/mlibc-${arch}"
  local cpu="$arch"
  local arch_flags=""
  case "$arch" in
    x86_64) cpu="x86_64" ;;
    aarch64) cpu="armv8-a" ;;
    riscv64) cpu="rv64gc"; arch_flags="$(printf "'%s', " $MYOS_RISCV64_ISA)'-mno-relax'," ;;
  esac

  # The cross file's C compiler, as a command.
  CROSS_CC=("$CLANG" "--target=$elf")
  [[ -n "$arch_flags" ]] && CROSS_CC+=($(printf '%s' "$arch_flags" | tr -d "',"))
  CROSS_CC+=(-ffreestanding -fPIC)

  echo "==> mlibc ($elf)"
  rm -rf "$build" "$prefix"
  mkdir -p "$build"
  # The cross file: clang for the bare target, as the newlib wrappers run it
  # (toolchain/newlib/tool-wrappers.sh), ld.lld for meson's link checks.
  cat > "$build/cross.txt" <<EOC
[binaries]
c = ['$CLANG', '--target=$elf', $arch_flags '-ffreestanding', '-fPIC']
cpp = ['$CLANGXX', '--target=$elf', $arch_flags '-ffreestanding', '-fPIC']
ar = '$AR_BIN'
strip = '$STRIP_BIN'

[built-in options]
c_args = ['-O2']
cpp_args = ['-O2']
c_link_args = ['-fuse-ld=lld', '-nostdlib']
cpp_link_args = ['-fuse-ld=lld', '-nostdlib']

[properties]
needs_exe_wrapper = true

[host_machine]
system = 'myos'
cpu_family = '$arch'
cpu = '$cpu'
endian = 'little'
EOC
  meson setup "$build" "$SRC" \
    --cross-file "$build/cross.txt" \
    --prefix "$prefix" --libdir lib --includedir include \
    --buildtype release \
    -Ddefault_library=static \
    -Dlibgcc_dependency=false \
    -Duse_freestnd_hdrs=enabled \
    -Dbuild_tests=false >"$build/setup.log" 2>&1 || { tail -40 "$build/setup.log" >&2; exit 1; }
  ninja -C "$build" >"$build/ninja.log" 2>&1 || { tail -60 "$build/ninja.log" >&2; exit 1; }
  ninja -C "$build" install >"$build/install.log" 2>&1 || { tail -40 "$build/install.log" >&2; exit 1; }
  build_builtins "$arch" "$prefix"
  echo "mlibc -> $prefix"
}

# lib/libbuiltins.a: the compiler-rt builtins (fetch.sh) a link against
# this libc.a needs, every one the compiler may call beside them: the IEEE
# binary128 `long double` of aarch64 and riscv64 (printf, strtod, libm),
# the complex multiplications of libm and the 128-bit integer divisions
# (`__int128` arithmetic: CPython's libmpdec) on every arch. Linked after libc.a
# (-lbuiltins).
build_builtins() {
  local arch="$1" prefix="$2"
  local obj="$ROOT/target/mlibc-build-$arch/builtins"
  local files=(muldc3.c mulsc3.c udivmodti4.c udivti3.c umodti3.c divti3.c modti3.c divmodti4.c)
  case "$arch" in
    aarch64|riscv64)
      files+=(addtf3.c subtf3.c multf3.c divtf3.c comparetf2.c
        extenddftf2.c extendsftf2.c trunctfdf2.c trunctfsf2.c
        fixtfdi.c fixtfsi.c fixunstfdi.c fixunstfsi.c
        floatditf.c floatsitf.c floatunditf.c floatunsitf.c multc3.c fe_stubs.c) ;;
  esac
  rm -rf "$obj"
  mkdir -p "$obj"
  # The binary128 helpers round to nearest and raise no exception flags
  # (the FPU's own mode and flags are for float and double).
  cat >"$obj/fe_stubs.c" <<'C'
int __fe_getround(void) { return 0; }
int __fe_raise_inexact(void) { return 0; }
C
  local f src
  for f in "${files[@]}"; do
    src="$CRT/$f"
    [[ "$f" == fe_stubs.c ]] && src="$obj/$f"
    "${CROSS_CC[@]}" -O2 -nostdinc -isystem "$prefix/include" \
      -isystem "$("$CLANG" -print-resource-dir)/include" -I"$CRT" \
      -c "$src" -o "$obj/${f%.c}.o"
  done
  "$AR_BIN" rcs "$prefix/lib/libbuiltins.a" "$obj"/*.o
}

build_one x86_64
build_one aarch64
build_one riscv64

myos_mlibc_version_hash > "$MYOS_MLIBC_VERSION"
