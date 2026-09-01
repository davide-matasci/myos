#!/usr/bin/env bash
# Fetch a pinned ripgrep release for the myos port (not vendored in-tree).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=ports/ripgrep/versions.env
source "$ROOT/ports/ripgrep/versions.env"
RG_SRC="$ROOT/target/ripgrep-src"
RG_TAG="${RIPGREP_TAG:-15.2.0}"
RG_REV="${RIPGREP_REV:-e89fff89ac9af12e8d4ce9d5fd07beb408ca730f}"

if [[ -d "$RG_SRC/.git" ]]; then
  got="$(git -C "$RG_SRC" rev-parse HEAD)"
  if [[ "$got" == "$RG_REV" ]]; then
    echo "ripgrep already present at $RG_SRC ($RG_TAG / $RG_REV)"
    exit 0
  fi
  echo "ripgrep at $got, want $RG_REV; refetching" >&2
fi

if [[ -e "$RG_SRC" ]]; then
  echo "removing incomplete ripgrep tree at $RG_SRC" >&2
  rm -rf "$RG_SRC"
fi

echo "==> fetch ripgrep ($RG_TAG / $RG_REV)"
git clone --depth 1 --branch "$RG_TAG" https://github.com/BurntSushi/ripgrep.git "$RG_SRC"
got="$(git -C "$RG_SRC" rev-parse HEAD)"
if [[ "$got" != "$RG_REV" ]]; then
  echo "error: ripgrep HEAD $got does not match pin $RG_REV" >&2
  exit 1
fi
