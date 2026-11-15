#!/usr/bin/env bash
# Fetch the Rust compiler's source (versions.env) into target/rustc-src:
# the tarball of the pinned nightly, checked against its sha256, without
# what a build of the compiler and cargo never reads (the test suite, the
# books, GCC, and the parts of llvm-project that are not LLVM).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=packages/rustc/versions.env
source "$HERE/versions.env"
SRC="$ROOT/target/rustc-src"
TAR="$ROOT/target/rustc-nightly-src-$RUST_NIGHTLY.tar.xz"

if [[ "$(cat "$SRC/.myos-fetched" 2>/dev/null)" == "$RUST_SRC_SHA256" ]]; then
  exit 0
fi
want="nightly-$RUST_NIGHTLY"
have="$(sed -n 's/^channel = "\(.*\)"/\1/p' "$ROOT/rust-toolchain.toml")"
if [[ "$have" != "$want" ]]; then
  echo "error: packages/rustc/versions.env pins $want, rust-toolchain.toml $have" >&2
  exit 1
fi
echo "==> fetch the Rust compiler's source ($want)"
if ! echo "$RUST_SRC_SHA256  $TAR" | sha256sum -c - >/dev/null 2>&1; then
  curl -fsSL --retry 4 -o "$TAR" "https://static.rust-lang.org/dist/$RUST_NIGHTLY/rustc-nightly-src.tar.xz"
  echo "$RUST_SRC_SHA256  $TAR" | sha256sum -c - >/dev/null
fi
rm -rf "$SRC"
mkdir -p "$SRC"
p=rustc-nightly-src
# Anchored: a vendored crate's own tests/ stays (cargo checks every file of
# a vendored crate against its checksum list).
tar -xJf "$TAR" -C "$SRC" --strip-components=1 --anchored \
  --exclude="$p/tests" --exclude="$p/src/gcc" --exclude="$p/src/doc" \
  --exclude="$p/src/llvm-project/llvm/test" --exclude="$p/src/llvm-project/clang" \
  --exclude="$p/src/llvm-project/lldb" --exclude="$p/src/llvm-project/flang" \
  --exclude="$p/src/llvm-project/mlir" --exclude="$p/src/llvm-project/polly" \
  --exclude="$p/src/llvm-project/bolt" --exclude="$p/src/llvm-project/libc" \
  --exclude="$p/src/llvm-project/openmp"
rm -f "$TAR"
printf '%s' "$RUST_SRC_SHA256" >"$SRC/.myos-fetched"
