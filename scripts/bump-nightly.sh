#!/usr/bin/env bash
# Try the next nightly against wire-myos.py and report whether a bump is safe.
# Does not modify rust-toolchain.toml unless --write is passed.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/myos-sysroot-lib.sh
source "$ROOT/scripts/myos-sysroot-lib.sh"

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
MYOS_NIGHTLY="$PROBE" "$ROOT/scripts/check-wire-myos.sh"

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
echo "Removed sysroot version stamp — run ./scripts/build-sysroot.sh next."
