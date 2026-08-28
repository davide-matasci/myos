#!/usr/bin/env bash
# Prepare a patched Rust library tree with myos std support.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUST_SRC="${RUST_SRC:-$(rustc +nightly-2026-07-26 --print sysroot)/lib/rustlib/src/rust/library}"
PATCH_DIR="${PATCH_DIR:-$ROOT/target/rust-std-patch/library}"

echo "Rust source:  $RUST_SRC"
echo "Patch output: $PATCH_DIR"

python3 "$ROOT/std/patches/wire-myos.py" "$PATCH_DIR" "$RUST_SRC"

MYOS_SYSROOT="${MYOS_SYSROOT:-$ROOT/target/myos-sysroot}"
TOOLCHAIN="$(rustc +nightly-2026-07-26 --print sysroot)"
rm -rf "$MYOS_SYSROOT"
cp -a "$TOOLCHAIN" "$MYOS_SYSROOT"
cp -a "$PATCH_DIR/." "$MYOS_SYSROOT/lib/rustlib/src/rust/library/"
chmod +x "$ROOT/scripts/myos-rustc.sh"

cat <<EOF

Next, build the hello example with:

  export RUSTC_BOOTSTRAP=1
  export MYOS_SYSROOT=$MYOS_SYSROOT
  export RUSTC=$ROOT/scripts/myos-rustc.sh
  cargo +nightly-2026-07-26 build -Z build-std=std,panic_abort -Z build-std-features=compiler-builtins-mem -Z unstable-options -Z json-target-spec \\
    --target $ROOT/targets/x86_64-unknown-myos.json \\
    --manifest-path $ROOT/std/examples/hello/Cargo.toml

EOF
