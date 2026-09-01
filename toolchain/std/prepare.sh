#!/usr/bin/env bash
# Prepare a patched Rust library tree with myos std support.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=toolchain/std/lib.sh
source "$ROOT/toolchain/std/lib.sh"

RUST_SRC="${RUST_SRC:-$(rustc +"$MYOS_NIGHTLY" --print sysroot)/lib/rustlib/src/rust/library}"
PATCH_DIR="${PATCH_DIR:-$ROOT/target/rust-std-patch/library}"

echo "Rust source:  $RUST_SRC"
echo "Patch output: $PATCH_DIR"

python3 "$ROOT/toolchain/std/patches/wire-myos.py" "$PATCH_DIR" "$RUST_SRC"

TOOLCHAIN="$(rustc +"$MYOS_NIGHTLY" --print sysroot)"
rm -rf "$MYOS_SYSROOT"
cp -a "$TOOLCHAIN" "$MYOS_SYSROOT"
cp -a "$PATCH_DIR/." "$MYOS_SYSROOT/lib/rustlib/src/rust/library/"
chmod +x "$ROOT/scripts/myos-rustc.sh"

for triple in "${MYOS_USER_TRIPLES[@]}"; do
  myos_install_target_spec "$triple"
done

cat <<EOF

Patched sysroot source tree: $MYOS_SYSROOT
Next: ./toolchain/std/build-sysroot.sh   # precompile std for both triples
      ./toolchain/std/build-std-hello.sh # build smoke binaries (uses sysroot)

EOF
