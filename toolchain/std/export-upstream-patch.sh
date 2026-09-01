#!/usr/bin/env bash
# Export a unified diff of libstd wiring + PAL overlays vs vanilla rust-src.
# Use when preparing a rust-lang/rust PR for target_os = "myos".
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=toolchain/std/lib.sh
source "$ROOT/toolchain/std/lib.sh"

RUST_SRC="${RUST_SRC:-$(rustc +"$MYOS_NIGHTLY" --print sysroot)/lib/rustlib/src/rust/library}"
VANILLA_DIR="${VANILLA_DIR:-$ROOT/target/upstream-export/vanilla/library}"
PATCHED_DIR="${PATCHED_DIR:-$ROOT/target/upstream-export/patched/library}"
OUT="${OUT:-$ROOT/target/myos-upstream-library.patch}"

mkdir -p "$(dirname "$OUT")"
rm -rf "$(dirname "$VANILLA_DIR")"
mkdir -p "$VANILLA_DIR"

echo "Copying vanilla library/ from $MYOS_NIGHTLY..."
cp -a "$RUST_SRC/." "$VANILLA_DIR/"

echo "Applying wire-myos.py..."
python3 "$ROOT/toolchain/std/patches/wire-myos.py" "$PATCHED_DIR" "$RUST_SRC"

echo "Writing unified diff -> $OUT"
if diff -ruN "$VANILLA_DIR" "$PATCHED_DIR" >"$OUT"; then
  echo "warning: no differences (patch file is empty)" >&2
else
  lines=$(wc -l <"$OUT")
  echo "exported $lines lines"
fi

echo ""
echo "Next: review $OUT and toolchain/std/upstream/README.md for the full rust-lang/rust checklist."
