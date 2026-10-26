#!/usr/bin/env bash
# Build LLVM's C++ runtimes against mlibc for the three arches, with
# LLVM's runtimes CMake: target/libcxx-<arch> holds include/c++/v1 (the
# headers, with this build's __config_site), lib/libc++.a (libc++abi
# merged in), libc++abi.a and libunwind.a. Static, with exceptions and
# RTTI (libunwind's DWARF unwinder finds the frames through mlibc's
# dl_iterate_phdr), threads on mlibc's pthreads. Then the C++ smoke
# against them (smoke.cpp, /bin/etc/mlibc/cxx_smoke).
#
# mlibc is neither glibc nor musl, and the target is myos's bare triple
# (<arch>-unknown-none, as every mlibc program's), which libc++ knows no
# platform for. mlibc has the POSIX locale functions (newlocale,
# uselocale, the *_l family), which libc++ then calls as they are; it
# takes its own ctype table (_LIBCPP_PROVIDES_DEFAULT_RUNE_TABLE, in
# __config_site for every program) and pthreads for its threads
# (LIBCXX_HAS_PTHREAD_API). No time zone database (no tzdata in the
# image).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=toolchain/libcxx/versions.env
source "$HERE/versions.env"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_libcxx_is_current; then
  echo "libc++ up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/mlibc/build.sh"
myos_ensure_llvm_bin

SRC="$ROOT/target/libcxx-src"
AR_BIN="$(command -v llvm-ar 2>/dev/null || echo ar)"
RANLIB_BIN="$(command -v llvm-ranlib 2>/dev/null || echo ranlib)"
CLANGXX="$(command -v clang++)"

build_arch() {
  local arch="$1"
  local build="$ROOT/target/libcxx-build-${arch}"
  local prefix="$ROOT/target/libcxx-${arch}"
  local flags
  flags="$(myos_mlibc_hosted_cflags "$arch") -D_GNU_SOURCE"

  echo "==> libc++ ($arch, mlibc)"
  rm -rf "$build" "$prefix"
  mkdir -p "$build"
  # Try-compiles as static libraries: nothing links before the runtimes
  # exist. The runtimes' own C++ headers come first in each compile, the
  # flags give mlibc's and clang's after them. CMake's platform is Linux,
  # for its UNIX logic (libc++abi's __cxa_thread_atexit is built only
  # there); the compiler's target stays myos's, so no __linux__ code.
  cmake -G Ninja -S "$SRC/runtimes" -B "$build" \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_INSTALL_PREFIX="$prefix" \
    -DCMAKE_SYSTEM_NAME=Linux \
    -DCMAKE_SYSTEM_PROCESSOR="$arch" \
    -DCMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY \
    -DCMAKE_C_COMPILER="$(command -v clang)" \
    -DCMAKE_CXX_COMPILER="$CLANGXX" \
    -DCMAKE_ASM_COMPILER="$(command -v clang)" \
    -DCMAKE_C_COMPILER_TARGET="${arch}-unknown-none" \
    -DCMAKE_CXX_COMPILER_TARGET="${arch}-unknown-none" \
    -DCMAKE_ASM_COMPILER_TARGET="${arch}-unknown-none" \
    -DCMAKE_AR="$AR_BIN" \
    -DCMAKE_RANLIB="$RANLIB_BIN" \
    -DCMAKE_C_FLAGS="$flags" \
    -DCMAKE_CXX_FLAGS="$flags" \
    -DCMAKE_ASM_FLAGS="$flags" \
    -DLLVM_ENABLE_RUNTIMES="libunwind;libcxxabi;libcxx" \
    -DLIBUNWIND_ENABLE_SHARED=OFF \
    -DLIBUNWIND_ENABLE_STATIC=ON \
    -DLIBUNWIND_USE_COMPILER_RT=ON \
    -DLIBUNWIND_INCLUDE_TESTS=OFF \
    -DLIBUNWIND_INCLUDE_DOCS=OFF \
    -DLIBCXXABI_ENABLE_SHARED=OFF \
    -DLIBCXXABI_ENABLE_STATIC=ON \
    -DLIBCXXABI_USE_LLVM_UNWINDER=ON \
    -DLIBCXXABI_ENABLE_STATIC_UNWINDER=ON \
    -DLIBCXXABI_USE_COMPILER_RT=ON \
    -DLIBCXXABI_HAS_PTHREAD_API=ON \
    -DLIBCXXABI_INCLUDE_TESTS=OFF \
    -DLIBCXX_ENABLE_SHARED=OFF \
    -DLIBCXX_ENABLE_STATIC=ON \
    -DLIBCXX_CXX_ABI=libcxxabi \
    -DLIBCXX_ENABLE_STATIC_ABI_LIBRARY=ON \
    -DLIBCXX_USE_COMPILER_RT=ON \
    -DLIBCXX_EXTRA_SITE_DEFINES=_LIBCPP_PROVIDES_DEFAULT_RUNE_TABLE \
    -DLIBCXX_HAS_PTHREAD_API=ON \
    -DLIBCXX_ENABLE_TIME_ZONE_DATABASE=OFF \
    -DLIBCXX_INCLUDE_BENCHMARKS=OFF \
    -DLIBCXX_INCLUDE_TESTS=OFF \
    -DLIBCXX_INCLUDE_DOCS=OFF \
    >"$build/cmake.log" 2>&1 || { tail -40 "$build/cmake.log" >&2; exit 1; }
  ninja -C "$build" install-unwind install-cxxabi install-cxx \
    >"$build/ninja.log" 2>&1 || { grep -m1 -B2 -A20 "error:" "$build/ninja.log" >&2 || tail -40 "$build/ninja.log" >&2; exit 1; }

  local out="$ROOT/target/cxx-smoke-mlibc-${arch}-unknown-none"
  # shellcheck disable=SC2046
  "$CLANGXX" $(myos_libcxx_cxxflags "$arch") -O2 -std=c++23 -c "$HERE/smoke.cpp" -o "$build/smoke.o"
  myos_libcxx_link "$arch" "$out" "$build/smoke.o"
  echo "libc++ -> $prefix, smoke -> $out"
}

for arch in ${MYOS_LIBCXX_ARCHES:-x86_64 aarch64 riscv64}; do
  build_arch "$arch"
done

echo "$(myos_libcxx_version_hash)" >"$MYOS_LIBCXX_VERSION"
echo "libc++ build ok ($LLVM_TAG)"
