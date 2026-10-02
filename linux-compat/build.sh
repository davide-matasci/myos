#!/usr/bin/env bash
# Build the optional Linux compatibility layer's userspace pieces, per arch
# (x86_64, aarch64, riscv64):
#   target/linux-launcher-<arch>-unknown-none  the `linux` launcher (myos newlib)
#   target/linux-smoke-<arch>-linux-musl       a Linux static-PIE test (musl)
# musl is built from its release tarball with clang for each target. Only
# needed for `cargo build --features linux_compat`; see docs/linux-compat.md.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

MUSL_VERSION=1.2.5
MUSL_SHA256=a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4
ARCHES=(x86_64 aarch64 riscv64)

myos_ensure_llvm_bin
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

tarball="$ROOT/target/linux-compat/musl-$MUSL_VERSION.tar.gz"
for arch in "${ARCHES[@]}"; do
  cc=(clang --target="$arch-linux-musl")
  prefix="$ROOT/target/linux-compat/musl-$MUSL_VERSION-$arch"

  # musl (static only) for the Linux-side test binaries.
  if [[ ! -f "$prefix/lib/libc.a" ]]; then
    mkdir -p "$ROOT/target/linux-compat"
    if [[ ! -f "$tarball" ]]; then
      curl -sSfL --retry 3 -o "$tarball.tmp" "https://musl.libc.org/releases/musl-$MUSL_VERSION.tar.gz"
      mv "$tarball.tmp" "$tarball"
    fi
    echo "$MUSL_SHA256  $tarball" | sha256sum -c -
    work="$ROOT/target/linux-compat/build-$arch"
    rm -rf "$work"
    mkdir -p "$work"
    tar xzf "$tarball" -C "$work"
    (
      cd "$work/musl-$MUSL_VERSION"
      CC="${cc[*]}" AR=llvm-ar RANLIB=llvm-ranlib ./configure --target="$arch-linux-musl" \
        --prefix="$prefix" --disable-shared >/dev/null
      make -j"$(nproc)" >/dev/null 2>&1
      make install >/dev/null
    )
    rm -rf "$work"
  fi

  echo "==> linux-smoke ($arch Linux, musl static-PIE)"
  "${cc[@]}" -static-pie -fPIE -O2 -nostdinc -nostdlib -fuse-ld=lld \
    -isystem "$prefix/include" -isystem "$(clang -print-resource-dir)/include" \
    -o "$ROOT/target/linux-smoke-$arch-linux-musl" \
    "$prefix/lib/rcrt1.o" "$prefix/lib/crti.o" \
    "$ROOT/linux-compat/tests/linux-smoke.c" \
    "$prefix/lib/libc.a" "$prefix/lib/crtn.o"

  echo "==> linux launcher ($arch, myos newlib)"
  triple="$arch-unknown-myos"
  nl="$ROOT/target/newlib-$arch"
  obj="$ROOT/target/linux-launcher-$arch.o"
  "${triple}-cc" -ffreestanding -fPIC -O2 -isystem "$nl/$triple/include" \
    -c "$ROOT/linux-compat/launcher.c" -o "$obj"
  ld.lld -pie --no-dynamic-linker -o "$ROOT/target/linux-launcher-$arch-unknown-none" \
    --entry=_start -z max-page-size=4096 \
    "$nl/$triple/lib/crt0.o" "$obj" -L"$nl/$triple/lib" \
    --start-group -lc -lgloss -lg --end-group
done
echo "linux-compat -> target/linux-launcher-*-unknown-none, target/linux-smoke-*-linux-musl"
