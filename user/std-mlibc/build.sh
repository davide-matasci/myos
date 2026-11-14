#!/usr/bin/env bash
# Build the std-on-mlibc demos (the `<arch>-unknown-myos-mlibc` targets).
# user/std-mlibc/mix, Rust and C in one program: its C half with clang
# against mlibc, the program with cargo against the prebuilt std of that
# flavour. user/std-mlibc/unwind: panics that unwind. Both linked with
# mlibc's libc.a and LLVM's libunwind (toolchain/libcxx):
# target/std-mlibc-{mix,unwind}-<triple>.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

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
done
myos_std_mlibc_version_hash >"$MYOS_STD_MLIBC_VERSION"
