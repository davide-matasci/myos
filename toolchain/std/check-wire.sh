#!/usr/bin/env bash
# Verify wire-myos.py applies cleanly to the pinned (or MYOS_NIGHTLY) rust-src tree.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=toolchain/std/lib.sh
source "$ROOT/toolchain/std/lib.sh"

RUST_SRC="${RUST_SRC:-$(rustc +"$MYOS_NIGHTLY" --print sysroot)/lib/rustlib/src/rust/library}"
CHECK_DIR="${CHECK_DIR:-$ROOT/target/wire-myos-check/library}"

echo "Checking wire-myos.py against $MYOS_NIGHTLY"
echo "  rust source: $RUST_SRC"

if [[ ! -d "$RUST_SRC/std" ]]; then
  echo "error: rust-src missing (install rust-src for $MYOS_NIGHTLY)" >&2
  exit 1
fi

python3 "$ROOT/toolchain/std/patches/wire-myos.py" "$CHECK_DIR" "$RUST_SRC"
echo "wire-myos.py: OK"
