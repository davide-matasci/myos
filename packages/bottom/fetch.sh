#!/usr/bin/env bash
# Fetch the pinned crates of the bottom package from crates.io, checked
# against their sha256, into target/bottom-crates/<name>-<version>.crate.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=packages/bottom/versions.env
source "$HERE/versions.env"

DIR="$ROOT/target/bottom-crates"
mkdir -p "$DIR"

# fetch_crate NAME VERSION SHA256
fetch_crate() {
  local name="$1" version="$2" sha="$3"
  local file="$DIR/$name-$version.crate"
  if [[ -f "$file" ]] && [[ "$(sha256sum "$file" | cut -d' ' -f1)" == "$sha" ]]; then
    return
  fi
  echo "==> fetch $name $version"
  curl -fsSL -o "$file.part" "https://static.crates.io/crates/$name/$name-$version.crate"
  local got
  got="$(sha256sum "$file.part" | cut -d' ' -f1)"
  if [[ "$got" != "$sha" ]]; then
    echo "$name-$version.crate sha256 mismatch: $got != $sha" >&2
    rm -f "$file.part"
    exit 1
  fi
  mv "$file.part" "$file"
}

fetch_crate bottom "$BOTTOM_VERSION" "$BOTTOM_SHA256"
fetch_crate crossterm "$CROSSTERM_VERSION" "$CROSSTERM_SHA256"
fetch_crate sysinfo "$SYSINFO_VERSION" "$SYSINFO_SHA256"
fetch_crate dirs-sys "$DIRS_SYS_VERSION" "$DIRS_SYS_SHA256"
fetch_crate parking_lot_core "$PARKING_LOT_CORE_VERSION" "$PARKING_LOT_CORE_SHA256"
echo "bottom crates ok ($DIR)"
