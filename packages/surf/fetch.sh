#!/usr/bin/env bash
# Fetch surf at its pinned commit into target/surf-src (idempotent; not
# vendored). The myos patch is applied by build.sh, on a fresh copy.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=packages/surf/versions.env
source "$HERE/versions.env"
SRC="$ROOT/target/surf-src"

if [[ -d "$SRC/.git" && -f "$SRC/surf.c" ]]; then
  got="$(git -C "$SRC" rev-parse HEAD)"
  if [[ "$got" == "$SURF_REV" ]]; then
    exit 0
  fi
  echo "surf at $got, want $SURF_REV; refetching" >&2
fi
rm -rf "$SRC"

echo "==> fetch surf ($SURF_VERSION)"
"$ROOT/scripts/git-retry.sh" clone "$SURF_URL" "$SRC"
git -C "$SRC" checkout -q "$SURF_REV"
echo "surf sources -> $SRC"
