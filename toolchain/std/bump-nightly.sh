#!/usr/bin/env bash
# Try the next nightly against wire-myos.py and report whether a bump is safe.
# Does not modify rust-toolchain.toml unless --write is passed.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=toolchain/std/lib.sh
source "$ROOT/toolchain/std/lib.sh"

WRITE=0
PROBE="${MYOS_PROBE_NIGHTLY:-nightly}"

for arg in "$@"; do
  case "$arg" in
    --write) WRITE=1 ;;
    --help|-h)
      echo "usage: bump-nightly.sh [--write] [nightly-date]"
      echo "  default probe: nightly (latest). Pass nightly-YYYY-MM-DD to test a specific build."
      exit 0
      ;;
    nightly*|beta*) PROBE="$arg" ;;
    *)
      echo "unknown argument: $arg" >&2
      exit 1
      ;;
  esac
done

echo "Pinned channel: $MYOS_NIGHTLY"
echo "Probing:        $PROBE"

rustup toolchain install "$PROBE" --component rust-src >/dev/null
MYOS_NIGHTLY="$PROBE" "$ROOT/toolchain/std/check-wire.sh"

echo ""
echo "wire-myos.py applies cleanly on $PROBE."

if [[ "$WRITE" -eq 0 ]]; then
  echo "Re-run with --write to set rust-toolchain.toml channel = \"$PROBE\" and invalidate the sysroot."
  exit 0
fi

file="$ROOT/rust-toolchain.toml"
sed -i "s/^channel = \".*\"/channel = \"$PROBE\"/" "$file"
echo "Updated $file"
rm -f "$MYOS_SYSROOT/.myos-sysroot-version"
echo "Removed sysroot version stamp — run ./toolchain/std/build-sysroot.sh next."
