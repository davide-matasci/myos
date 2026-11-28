#!/bin/sh
# Prepare a patched Rust library tree with myos std support.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=toolchain/std/lib.sh
. "$ROOT/toolchain/std/lib.sh"

PATCH_DIR="${PATCH_DIR:-$ROOT/target/rust-std-patch/library}"

# On myos (scripts/self-host.sh) the patched library is the tree's (the
# myos-src package's, as the lines below left it where the package was
# built), and the sysroot is it and the rustc app's own libraries: what a
# build script is compiled against, and the linker.
if myos_self_host; then
  [ -f "$PATCH_DIR/Cargo.lock" ] || { echo "error: no patched library at $PATCH_DIR (the myos-src package's tree has it)" >&2; exit 1; }
  real="$(rustc --print sysroot)"
  host="$(rustc -vV | sed -n 's/^host: //p')"
  rm -rf "$MYOS_SYSROOT"
  mkdir -p "$MYOS_SYSROOT/lib/rustlib/src/rust"
  ln -s "$PATCH_DIR" "$MYOS_SYSROOT/lib/rustlib/src/rust/library"
  ln -s "$real/lib/rustlib/$host" "$MYOS_SYSROOT/lib/rustlib/$host"
  for triple in $MYOS_SYSROOT_TRIPLES; do
    myos_install_target_spec "$triple"
  done
  exit 0
fi

RUST_SRC="${RUST_SRC:-$(rustc +"$MYOS_NIGHTLY" --print sysroot)/lib/rustlib/src/rust/library}"

echo "Rust source:  $RUST_SRC"
echo "Patch output: $PATCH_DIR"

python3 "$ROOT/toolchain/std/patches/wire-myos.py" "$PATCH_DIR" "$RUST_SRC"

TOOLCHAIN="$(rustc +"$MYOS_NIGHTLY" --print sysroot)"
rm -rf "$MYOS_SYSROOT"
cp -a "$TOOLCHAIN" "$MYOS_SYSROOT"
cp -a "$PATCH_DIR/." "$MYOS_SYSROOT/lib/rustlib/src/rust/library/"
chmod +x "$ROOT/scripts/myos-rustc.sh"

for triple in $MYOS_SYSROOT_TRIPLES; do
  myos_install_target_spec "$triple"
done

cat <<EOF

Patched sysroot source tree: $MYOS_SYSROOT
Next: ./toolchain/std/build-sysroot.sh   # precompile std for both triples
      ./user/std/build.sh # build smoke binaries (uses sysroot)

EOF
