#!/usr/bin/env bash
# Fetch a pinned TinyCC revision for the myos port.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=ports/tcc/versions.env
source "$HERE/versions.env"
TCC_SRC="$ROOT/target/tcc-src"
TCC_REV="${TCC_REV:-2ba12e83b3599ca8f5d50c179fe5138fe956f0c9}"

# Also require include/stddef.h: an empty include/ stub (e.g. from
# packing target/tcc-* into ci-build.tar) must not count as present.
if [[ -d "$TCC_SRC/.git" && -f "$TCC_SRC/tcc.c" && -f "$TCC_SRC/include/stddef.h" ]]; then
  got="$(git -C "$TCC_SRC" rev-parse HEAD)"
  if [[ "$got" == "$TCC_REV" ]]; then
    echo "tinycc already present at $TCC_SRC ($TCC_REV)"
    exit 0
  fi
  echo "tinycc at $got, want $TCC_REV; refetching" >&2
  rm -rf "$TCC_SRC"
fi

if [[ -e "$TCC_SRC" ]]; then
  echo "removing incomplete tinycc tree at $TCC_SRC" >&2
  rm -rf "$TCC_SRC"
fi

echo "==> fetch tinycc ($TCC_REV)"
"$ROOT/scripts/git-retry.sh" clone --depth 1 https://github.com/TinyCC/tinycc.git "$TCC_SRC"
"$ROOT/scripts/git-retry.sh" -C "$TCC_SRC" fetch --depth 1 origin "$TCC_REV"
git -C "$TCC_SRC" checkout "$TCC_REV"
got="$(git -C "$TCC_SRC" rev-parse HEAD)"
if [[ "$got" != "$TCC_REV" ]]; then
  echo "error: tinycc HEAD $got does not match pin $TCC_REV" >&2
  exit 1
fi
