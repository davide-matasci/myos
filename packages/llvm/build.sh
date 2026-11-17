#!/bin/sh
# Cross-build LLVM's compiler for myos against mlibc and libc++ (static),
# for the arches MYOS_LLVM_ARCHES lists (all three by default), into
# target/llvm-<arch>:
#
#   bin/llvm            clang, lld and the binutils in one program (LLVM's
#                       multicall driver: the tool is the name it runs as)
#   lib/clang/19        clang's own headers (its resource directory)
#   lib/mlibc           what a myos program is built against: include/
#                       (mlibc's headers, libc++'s in c++/v1) and lib/
#                       (crt0.o, libc.a and its companions, the compiler
#                       builtins, libc++.a, libunwind.a), from the mlibc
#                       and libcxx ports
#
# The compiler knows myos (llvm-myos.myos.patch): the `<arch>-unknown-myos`
# triple, a target that is the arch's generic ELF one with __myos__
# defined, and a driver toolchain that finds lib/mlibc and links a static
# PIE with ld.lld the way myos_libcxx_link does. Every backend a myos
# machine has is built in (X86, AArch64, RISC-V), so one compiler builds
# for the three.
#
# LLVM's table generators run on the build machine: its CMake builds them
# with the build machine's compiler (the NATIVE tree) beside the cross
# build. The build tree is kept until the build succeeds, so a failed run
# goes on where it stopped.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
. "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/llvm/port.env
. "$HERE/port.env" # PORT_LIBC: the libc myos_write_cross_cc links

if myos_llvm_is_current; then
  echo "llvm up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/mlibc/build.sh"
"$ROOT/toolchain/libcxx/build.sh"
myos_ensure_llvm_bin
# The binutils: newlib's tool wrappers, <triple>-ar and -strip, over
# LLVM's (as packages/glib).
export PATH="$ROOT/target/newlib-bin:$PATH"
for tool in cmake ninja python3 patch; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "error: $tool not found (README.md)" >&2
    exit 1
  fi
done

T="$ROOT/target"
SRC="$T/llvm-src"

# The myos patches, on the pristine checkout; once per set of patches, so
# that a second run does not touch the sources (and rebuild everything).
patches="$(cat "$HERE"/*.myos.patch | sha256sum | cut -d' ' -f1)"
if [ "$(cat "$SRC/.myos-patched" 2>/dev/null)" != "$patches" ]; then
  git -C "$SRC" checkout -q -- .
  git -C "$SRC" clean -q -fd -- llvm clang lld
  for p in "$HERE"/*.myos.patch; do
    patch -d "$SRC" -p1 -s <"$p"
  done
  printf '%s' "$patches" >"$SRC/.myos-patched"
fi

build_arch() (
  # The build tree is one level down: clang's headers are written to its
  # resource directory as the compiler will look for it, two levels above
  # its bin/ (CLANG_RESOURCE_DIR), which is then $work/lib/clang/19.
  arch="$1" out="$T/llvm-$1" work="$T/llvm-build-$1" build="$T/llvm-build-$1/build" off=""
  mkdir -p "$build/tc"
  myos_write_cross_cc "$arch" "$build/tc/cc"
  myos_write_cross_cxx "$arch" "$build/tc/c++"
  # The links' response files (@file), spelled out for the cross driver,
  # which takes flags only on its command line.
  cat >"$build/tc/link" <<'EOL'
#!/bin/sh
n=$#
set -f
for a in "$@"; do
  case "$a" in
    @*) set -- "$@" $(cat "${a#@}") ;;
    *) set -- "$@" "$a" ;;
  esac
done
shift "$n"
exec "$@"
EOL
  chmod +x "$build/tc/link"
  # "Linux" for CMake: a Unix with ELF. The compiler says what the system
  # is (__myos__, below), not __linux__.
  cat >"$build/tc/toolchain.cmake" <<EOT
set(CMAKE_SYSTEM_NAME Linux)
set(CMAKE_SYSTEM_PROCESSOR $arch)
set(CMAKE_C_COMPILER $build/tc/cc)
set(CMAKE_CXX_COMPILER $build/tc/c++)
set(CMAKE_AR $(command -v "${arch}-unknown-myos-ar"))
set(CMAKE_RANLIB $(command -v "${arch}-unknown-myos-ranlib"))
set(CMAKE_FIND_ROOT_PATH $T/mlibc-$arch)
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)
set(CMAKE_FIND_LIBRARY_SUFFIXES .a)
set(CMAKE_C_LINKER_LAUNCHER $build/tc/link)
set(CMAKE_CXX_LINKER_LAUNCHER $build/tc/link)
EOT
  # The driver's tools nothing on myos uses: the Mach-O, COFF and
  # WebAssembly ones, the profile and debug-info servers.
  for t in SANCOV LLVM_RC LLVM_PROFDATA LLVM_MT LLVM_ML LLVM_LIPO LLVM_LIBTOOL_DARWIN LLVM_IFS \
    LLVM_GSYMUTIL LLVM_DWP LLVM_DEBUGINFOD DSYMUTIL; do
    off="$off -DLLVM_TOOL_${t}_BUILD=OFF"
  done
  # __myos__, which this cross compiler (the build machine's clang, for a
  # bare target) does not define and the compiler being built does; and
  # <endian.h> first, for BYTE_ORDER, which LLVM looks for in a BSD's
  # <machine/endian.h> on a system it does not know.
  flags="-D__myos__ -include endian.h"
  # Built for size: the program is one static binary of every tool.
  # No sigaltstack on myos: LLVM's crash handlers run on the thread's stack.
  # clang's resource directory is /lib/clang/19 for /bin/custom/clang.
  # shellcheck disable=SC2086
  cmake -G Ninja -S "$SRC/llvm" -B "$build" \
    -DCMAKE_TOOLCHAIN_FILE="$build/tc/toolchain.cmake" -DCMAKE_BUILD_TYPE=MinSizeRel \
    -DCMAKE_C_FLAGS="$flags" -DCMAKE_CXX_FLAGS="$flags" \
    -DLLVM_ENABLE_PROJECTS="clang;lld" -DLLVM_TARGETS_TO_BUILD="X86;AArch64;RISCV" \
    -DLLVM_HOST_TRIPLE="$arch-unknown-myos" -DLLVM_DEFAULT_TARGET_TRIPLE="$arch-unknown-myos" \
    -DLLVM_TOOL_LLVM_DRIVER_BUILD=ON $off -DCLANG_TOOL_CLANG_SCAN_DEPS_BUILD=OFF \
    -DCLANG_RESOURCE_DIR="../../lib/clang/19" \
    -DLLVM_ENABLE_ZLIB=OFF -DLLVM_ENABLE_ZSTD=OFF -DLLVM_ENABLE_LIBXML2=OFF \
    -DLLVM_ENABLE_LIBEDIT=OFF -DLLVM_ENABLE_LIBPFM=OFF -DLLVM_ENABLE_PLUGINS=OFF -DLLVM_ENABLE_BACKTRACES=OFF \
    -DLLVM_BUILD_LLVM_DYLIB=OFF -DLLVM_LINK_LLVM_DYLIB=OFF -DBUILD_SHARED_LIBS=OFF \
    -DLLVM_INCLUDE_TESTS=OFF -DLLVM_INCLUDE_BENCHMARKS=OFF -DLLVM_INCLUDE_EXAMPLES=OFF -DLLVM_INCLUDE_DOCS=OFF \
    -DLLVM_INCLUDE_UTILS=OFF -DLLVM_ENABLE_BINDINGS=OFF -DLLVM_ENABLE_OCAMLDOC=OFF \
    -DCLANG_ENABLE_STATIC_ANALYZER=OFF -DCLANG_ENABLE_ARCMT=OFF -DCLANG_PLUGIN_SUPPORT=OFF \
    -DCLANG_INCLUDE_TESTS=OFF -DCLANG_INCLUDE_DOCS=OFF -DCLANG_BUILD_EXAMPLES=OFF \
    -DLLVM_ENABLE_PIC=ON -DLLVM_ENABLE_THREADS=ON -DHAVE_SIGALTSTACK=OFF \
    >"$build/cmake.log" 2>&1 || { tail -40 "$build/cmake.log" >&2; exit 1; }
  ninja -C "$build" -j"$(nproc)" llvm-driver clang-resource-headers >"$build/ninja.log" 2>&1 \
    || { grep -n "error" "$build/ninja.log" | head -30 >&2 || true; tail -20 "$build/ninja.log" >&2; exit 1; }

  rm -rf "$out"
  mkdir -p "$out/bin" "$out/lib/clang" "$out/lib/mlibc/lib"
  cp "$build/bin/llvm" "$out/bin/llvm"
  "${arch}-unknown-myos-strip" -s "$out/bin/llvm"
  cp -r "$work/lib/clang/19" "$out/lib/clang/19"
  # The development tree: mlibc's and libc++'s, as their ports built them.
  cp -r "$T/mlibc-$arch/include" "$out/lib/mlibc/include"
  mkdir -p "$out/lib/mlibc/include/c++"
  cp -r "$T/libcxx-$arch/include/c++/v1" "$out/lib/mlibc/include/c++/v1"
  cp "$T/mlibc-$arch/lib/"*.o "$T/mlibc-$arch/lib/"*.a "$out/lib/mlibc/lib/"
  cp "$T/libcxx-$arch/lib/libc++.a" "$T/libcxx-$arch/lib/libunwind.a" "$out/lib/mlibc/lib/"
  rm -rf "$work"
)

# The arches MYOS_LLVM_ARCHES lists, a stamp each: CI builds one per job
# (PORT_ARCH_JOBS).
hash="$(myos_llvm_version_hash)"
for arch in ${MYOS_LLVM_ARCHES:-x86_64 aarch64 riscv64}; do
  if (MYOS_LLVM_ARCHES="$arch" && myos_llvm_is_current); then
    echo "llvm ($arch) up to date"
    continue
  fi
  echo "==> llvm ($arch-unknown-myos)"
  build_arch "$arch"
  echo "$hash" >"$T/.myos-llvm-$arch-version"
  echo "llvm ($arch) -> $T/llvm-$arch"
done
