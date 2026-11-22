#!/bin/sh
# Build the std-on-mlibc demos (the `<arch>-unknown-myos-mlibc` targets).
# user/std-mlibc/mix, Rust and C in one program: its C half with clang
# against mlibc, the program with cargo against the prebuilt std of that
# flavour. user/std-mlibc/unwind: panics that unwind. user/std-mlibc/unix:
# what std's unix code asks of the C library and the kernel. All linked
# with mlibc's libc.a and LLVM's libunwind (toolchain/libcxx):
# target/std-mlibc-{mix,unwind,unix}-<triple>.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
. "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_std_mlibc_is_current; then
  echo "std mlibc demo up to date"
  exit 0
fi
"$ROOT/toolchain/std/fetch-sysroot.sh"
"$ROOT/toolchain/mlibc/build.sh"
"$ROOT/toolchain/libcxx/build.sh"
myos_ensure_llvm_bin
AR_BIN="$(command -v llvm-ar 2>/dev/null || echo ar)"

for arch in x86_64 aarch64 riscv64; do
  triple="${arch}-unknown-myos-mlibc"
  target_dir="$ROOT/target/std-mlibc-mix-build-${triple}"
  echo "==> std-mlibc-mix ($triple)"
  myos_cargo_target_dir "$triple" "$target_dir"
  # shellcheck disable=SC2046
  clang $(myos_mlibc_cflags "$arch") -O2 -c "$HERE/mix/mix.c" -o "$target_dir/mix.o"
  rm -f "$target_dir/libmixc.a"
  "$AR_BIN" rcs "$target_dir/libmixc.a" "$target_dir/mix.o"
  # Where the target's link finds -lc, -lbuiltins and -lunwind.
  libs="-L native=$ROOT/target/mlibc-${arch}/lib -L native=$ROOT/target/libcxx-${arch}/lib"
  RUSTFLAGS="$libs -L native=$target_dir -l static=mixc" \
    myos_cargo_build_app "$triple" release "$target_dir" "$HERE/mix/Cargo.toml" std-mlibc-mix
  cp "$target_dir/${triple}/release/std-mlibc-mix" "$ROOT/target/std-mlibc-mix-${triple}"
  echo "==> std-mlibc-unwind ($triple)"
  unwind_dir="$ROOT/target/std-mlibc-unwind-build-${triple}"
  RUSTFLAGS="$libs" \
    myos_cargo_build_app "$triple" release "$unwind_dir" "$HERE/unwind/Cargo.toml" std-mlibc-unwind
  cp "$unwind_dir/${triple}/release/std-mlibc-unwind" "$ROOT/target/std-mlibc-unwind-${triple}"
  echo "==> std-mlibc-unix ($triple)"
  unix_dir="$ROOT/target/std-mlibc-unix-build-${triple}"
  RUSTFLAGS="$libs" \
    myos_cargo_build_app "$triple" release "$unix_dir" "$HERE/unix/Cargo.toml" std-mlibc-unix
  cp "$unix_dir/${triple}/release/std-mlibc-unix" "$ROOT/target/std-mlibc-unix-${triple}"
  # Rust shared objects (dylib/: librt.so, which holds std, libplugin.so
  # and the program over both): the same sysroot, its rlibs linked into
  # librt.so; libc.so in place of libc.a (mlibc's shared/ before its
  # lib/), the dynamic linker named, the objects found beside the
  # program. The program's entry is mlibc's crt0.o, an argument of its
  # link alone (`cargo rustc`: in RUSTFLAGS it would be in the shared
  # objects too; a build script would need a std for the build machine,
  # which CI's toolchain does not carry).
  echo "==> std-mlibc-dylib ($triple)"
  dylib_dir="$ROOT/target/std-mlibc-dylib-build-${triple}"
  myos_export_toolchain_env
  myos_cargo_target_dir "$triple" "$dylib_dir"
  dylib_flags="-C prefer-dynamic -L native=$ROOT/target/mlibc-${arch}/shared $libs -C link-arg=--dynamic-linker=/lib/ld.so -C link-arg=-rpath=\$ORIGIN"
  set -- --release -Z unstable-options -Z json-target-spec \
    --target "$ROOT/targets/${triple}.json" --target-dir "$dylib_dir" \
    --manifest-path "$HERE/dylib/Cargo.toml"
  RUSTFLAGS="$dylib_flags" cargo "+$MYOS_NIGHTLY" build "$@" -p std-mlibc-plugin
  RUSTFLAGS="$dylib_flags" cargo "+$MYOS_NIGHTLY" rustc "$@" -p std-mlibc-dylib \
    --bin std-mlibc-dylib -- -C "link-arg=$ROOT/target/mlibc-${arch}/lib/crt0.o"
  out="$dylib_dir/${triple}/release"
  cp "$out/std-mlibc-dylib" "$ROOT/target/std-mlibc-dylib-${triple}"
  cp "$out/librt.so" "$ROOT/target/std-mlibc-dylib-rt-${triple}"
  cp "$out/libplugin.so" "$ROOT/target/std-mlibc-dylib-plugin-${triple}"
done
myos_std_mlibc_version_hash >"$MYOS_STD_MLIBC_VERSION"
