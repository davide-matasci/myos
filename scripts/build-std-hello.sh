#!/usr/bin/env bash
# Build the patched std hello binary and copy it to the workspace target/ path
# the x86_64 kernel embeds as /stdhello.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

"$ROOT/scripts/prepare-rust-std-myos.sh" >/dev/null

export RUSTC_BOOTSTRAP=1
export MYOS_SYSROOT="${MYOS_SYSROOT:-$ROOT/target/myos-sysroot}"
export RUSTC="$ROOT/scripts/myos-rustc.sh"

cargo +nightly-2026-07-26 build --release \
  -Z build-std=std,panic_abort \
  -Z build-std-features=compiler-builtins-mem \
  -Z unstable-options -Z json-target-spec \
  --target "$ROOT/targets/x86_64-unknown-myos.json" \
  --manifest-path "$ROOT/std/examples/hello/Cargo.toml"

mkdir -p "$ROOT/target"
cp "$ROOT/std/examples/hello/target/x86_64-unknown-myos/release/std-hello" \
  "$ROOT/target/std-hello-x86_64-unknown-myos"

echo "std-hello -> $ROOT/target/std-hello-x86_64-unknown-myos"
