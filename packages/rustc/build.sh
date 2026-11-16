#!/usr/bin/env bash
# Cross-build the Rust compiler and cargo with myos as their host, for the
# arches MYOS_RUSTC_ARCHES lists (all three by default), into
# target/rustc-<arch>:
#
#   bin/rustc, bin/cargo
#   lib/librustc_driver-*.so        the compiler, LLVM in it
#   lib/rustlib/<triple>/lib        the std of <arch>-unknown-myos-mlibc
#   lib/rustlib/<triple>/bin/rust-lld   the linker rustc runs: a script
#                                   over the llvm app's lld (files/)
#
# The source is the pinned nightly's (fetch.sh), with myos in it
# (README.md): rust-myos.myos.patch on the compiler, the std's own wiring
# on library/ (toolchain/std/patches), port-crates.py on the vendored
# crates. Its own build system does the rest, in one tree
# (target/rustc-build):
#
#   stage 0   the beta compiler its stage0 file names, downloaded
#   stage 1   the compiler for the build machine, from this source: the
#             first one that knows the myos targets. Its LLVM is the one
#             Rust's CI built for this commit, downloaded too
#   std       for <arch>-unknown-myos-mlibc, by stage 1
#   stage 2   the compiler and cargo for myos, by stage 1, linked against
#             that std and the llvm package's LLVM
#
# LLVM for myos is the llvm package's (packages/llvm: upstream LLVM of
# the major this compiler is written for, not the fork its source
# carries): the libraries that port keeps beside its program, static,
# against mlibc and libc++ (target/llvm-<arch>/dev), with their
# llvm-config for the build machine. cargo's TLS is the mlibc build of
# LibreSSL; curl, libgit2, libssh2 and zlib are the copies their crates
# carry.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/rustc/port.env
source "$HERE/port.env" # PORT_LIBC: the libc myos_write_cross_cc links
# shellcheck source=packages/rustc/versions.env
source "$HERE/versions.env"

if myos_rustc_is_current; then
  echo "rustc up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/mlibc/build.sh"
"$ROOT/toolchain/libcxx/build.sh"
"$ROOT/ports/libressl/build.sh"
MYOS_LLVM_ARCHES="${MYOS_RUSTC_ARCHES:-x86_64 aarch64 riscv64}" "$ROOT/packages/llvm/build.sh"
myos_ensure_llvm_bin
# The binutils: newlib's tool wrappers, <triple>-ar and -strip, over
# LLVM's (as packages/llvm).
export PATH="$ROOT/target/newlib-bin:$PATH"
for tool in python3 patch curl; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "error: $tool not found (README.md)" >&2
    exit 1
  fi
done

T="$ROOT/target"
SRC="$T/rustc-src"
BUILD="$T/rustc-build"
STD="$ROOT/toolchain/std"
# The pinned nightly's lld, for the links of myos programs on the build
# machine (the std port's linker too, targets/*.json).
RUST_LLD="$(dirname "$(rustc "+$MYOS_NIGHTLY" --print target-libdir)")/bin/rust-lld"
[[ -x "$RUST_LLD" ]] || { echo "error: no rust-lld in the $MYOS_NIGHTLY toolchain" >&2; exit 1; }

# myos in the source, on the pristine tree: once per set of patches, so
# that a second run does not touch the sources (and rebuild everything).
prep="$({ cat "$HERE/rust-myos.myos.patch" "$HERE/port-crates.py"; myos_sysroot_version_hash; } | sha256sum | cut -d' ' -f1)"
if [[ "$(cat "$SRC/.myos-patched" 2>/dev/null)" != "$prep" ]]; then
  if [[ -e "$SRC/.myos-patched" ]]; then
    rm -f "$SRC/.myos-fetched"
    "$HERE/fetch.sh"
  fi
  echo "==> rustc: myos in the source"
  patch -d "$SRC" -p1 -s <"$HERE/rust-myos.myos.patch"
  mv "$SRC/library" "$SRC/library.pristine"
  python3 "$STD/patches/wire-myos.py" "$SRC/library" "$SRC/library.pristine" >/dev/null
  rm -rf "$SRC/library.pristine"
  python3 "$HERE/port-crates.py" "$SRC/vendor" "$STD/patches/libc-myos.patch" >/dev/null
  printf '%s' "$prep" >"$SRC/.myos-patched"
fi

# write_toolchain ARCH DIR: the cross tools of one arch.
write_toolchain() {
  local arch="$1" tc="$2" c
  mkdir -p "$tc"
  myos_write_cross_cc "$arch" "$tc/cc.real"
  myos_write_cross_cxx "$arch" "$tc/c++.real"
  for c in cc c++; do
    cat >"$tc/$c" <<EOF
#!/usr/bin/env bash
# The cross compiler behind a filter: a build script's cc crate names the
# Rust target (--target=$arch-unknown-myos-mlibc), which this clang does
# not know; the driver below names the one it compiles for.
args=()
for a in "\$@"; do
  case "\$a" in
    --target=*-myos-mlibc | --target=*-myos) ;;
    *) args+=("\$a") ;;
  esac
done
exec "$tc/$c.real" "\${args[@]}"
EOF
    chmod +x "$tc/$c"
  done
  # rustc's linker for the target on the build machine, as files/rust-lld
  # is on myos: lld with where mlibc and LLVM's runtimes are.
  cat >"$tc/ld" <<EOF
#!/usr/bin/env bash
# A shared object and a program over shared objects link the shared
# mlibc; such a program starts through /lib/ld.so and takes its entry
# from mlibc's crt0.o (std's own is in the shared object that holds std).
# Any other program is a static PIE.
mlibc="$T/mlibc-$arch"
cxx="$T/libcxx-$arch"
# The build system writes some arguments for a compiler driver
# (-Wl,a,b): they are the linker's a b.
shared=0 dynamic=0 args=()
for a in "\$@"; do
  case "\$a" in
    -shared) shared=1 ;;
    *.so) dynamic=1 ;;
  esac
  if [[ "\$a" == -Wl,-z,origin ]]; then
    # \$ORIGIN in a RUNPATH needs no flag, and mlibc's ld.so says so of
    # one it does not know at every start.
    continue
  elif [[ "\$a" == -Wl,* ]]; then
    IFS=, read -ra parts <<<"\${a#-Wl,}"
    args+=("\${parts[@]}")
  else
    args+=("\$a")
  fi
done
set -- "\${args[@]}"
if ((shared)); then
  exec "$RUST_LLD" -flavor gnu "\$@" --hash-style=gnu -L"\$mlibc/shared" -L"\$mlibc/lib" -L"\$cxx/lib"
elif ((dynamic)); then
  exec "$RUST_LLD" -flavor gnu "\$@" "\$mlibc/lib/crt0.o" --dynamic-linker=/lib/ld.so --export-dynamic \\
    --hash-style=gnu -L"\$mlibc/shared" -L"\$mlibc/lib" -L"\$cxx/lib"
else
  # No dynamic linker: lld then settles the weak symbols a static
  # program leaves undefined itself.
  exec "$RUST_LLD" -flavor gnu "\$@" --no-dynamic-linker -L"\$mlibc/lib" -L"\$cxx/lib"
fi
EOF
  chmod +x "$tc/ld"
}

build_arch() {
  local arch="$1" triple="$1-unknown-myos-mlibc" out="$T/rustc-$1" tc="$T/rustc-tc-$1" llvm="$T/llvm-$1/dev"
  local host stage2 std tools var f b
  host="$(rustc "+$MYOS_NIGHTLY" -vV | sed -n 's/^host: //p')"
  write_toolchain "$arch" "$tc"

  mkdir -p "$BUILD"
  cat >"$BUILD/bootstrap-$arch.toml" <<EOF
# Written by packages/rustc/build.sh.
change-id = "ignore"

[build]
build = "$host"
build-dir = "$BUILD"
# Everything from the tarball's vendor/: no network but for stage 0 and
# the build machine's LLVM.
vendor = true
locked-deps = true
docs = false
submodules = false
extended = false
# The builtins in Rust: the C ones would be compiled for a target this
# clang does not know.
optimized-compiler-builtins = false

[llvm]
# For the build machine's compiler (stage 1): the LLVM Rust's CI built for
# this commit.
download-ci-llvm = true

[rust]
channel = "nightly"
download-rustc = false
debug = false
debug-assertions = false
incremental = false
# The patched crates warn in places.
deny-warnings = false
lld = false
llvm-tools = false

[target.$triple]
cc = "$tc/cc"
cxx = "$tc/c++"
ar = "$(command -v "${arch}-unknown-myos-ar")"
ranlib = "$(command -v "${arch}-unknown-myos-ranlib")"
linker = "$tc/ld"
llvm-config = "$llvm/bin/llvm-config"
EOF
  echo "==> rustc ($triple)"
  # cargo's TLS: the mlibc build of LibreSSL (openssl-sys reads
  # <TRIPLE>_OPENSSL_DIR). Not the std port's own cargo settings.
  var="$(printf '%s' "$triple" | tr '[:lower:]-' '[:upper:]_')"
  (
    cd "$SRC"
    env -u RUSTC -u RUSTFLAGS -u MYOS_SYSROOT -u RUSTC_BOOTSTRAP \
      "${var}_OPENSSL_DIR=$T/libressl-mlibc-$arch" "${var}_OPENSSL_STATIC=1" \
      python3 x.py build --config "$BUILD/bootstrap-$arch.toml" --stage 2 \
      compiler/rustc src/tools/cargo --host "$triple" --target "$triple"
  ) >"$BUILD/build-$arch.log" 2>&1 \
    || { grep -n "^error" -A12 "$BUILD/build-$arch.log" | head -80 >&2 || true; tail -20 "$BUILD/build-$arch.log" >&2; exit 1; }

  stage2="$BUILD/$triple/stage2"
  std="$BUILD/$host/stage1/lib/rustlib/$triple/lib"
  tools="$BUILD/$host/stage2-tools/$triple/release"
  rm -rf "$out"
  mkdir -p "$out/bin" "$out/lib/rustlib/$triple/lib" "$out/lib/rustlib/$triple/bin"
  cp "$stage2/bin/rustc" "$tools/cargo" "$out/bin/"
  cp "$stage2"/lib/librustc_driver-*.so "$out/lib/"
  "${arch}-unknown-myos-strip" -s "$out/bin/rustc" "$out/bin/cargo" "$out"/lib/librustc_driver-*.so
  # The std: each crate's rlib and the metadata beside it (the rlib holds
  # a stub). Not the shared libstd (a program links std's rlibs), nor the
  # metadata of the compiler's own crates, which is in the same directory.
  for f in "$std"/*.rlib; do
    b="$(basename "$f" .rlib)"
    cp "$f" "$out/lib/rustlib/$triple/lib/"
    [[ ! -e "$std/$b.rmeta" ]] || cp "$std/$b.rmeta" "$out/lib/rustlib/$triple/lib/"
  done
  cp "$HERE/files/rust-lld" "$out/lib/rustlib/$triple/bin/rust-lld"
  chmod +x "$out/lib/rustlib/$triple/bin/rust-lld"
  echo "rustc ($arch) -> $out"
}

# The arches MYOS_RUSTC_ARCHES lists, a stamp each: CI builds one per job
# (PORT_ARCH_JOBS).
hash="$(myos_rustc_version_hash)"
for arch in ${MYOS_RUSTC_ARCHES:-x86_64 aarch64 riscv64}; do
  if MYOS_RUSTC_ARCHES="$arch" myos_rustc_is_current; then
    continue
  fi
  build_arch "$arch"
  printf '%s\n' "$hash" >"$T/.myos-rustc-$arch-version"
done
