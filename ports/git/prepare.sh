#!/usr/bin/env bash
# Prepare a myos build tree from fetched upstream Git (config.mak + stubs + patches).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
"$ROOT/ports/git/fetch.sh"

GIT="$ROOT/target/git-src"
WORK="$ROOT/target/git-myos-build"
MYOS="$ROOT/ports/git"

rm -rf "$WORK"
mkdir -p "$WORK"

rsync -a \
  --exclude='.git' \
  --exclude='ci' \
  --exclude='.github' \
  --exclude='templates' \
  --exclude='contrib' \
  --exclude='gitweb/static' \
  --exclude='gitk-git' \
  --exclude='git-gui' \
  "$GIT/" "$WORK/"

cp "$MYOS/config.mak" "$WORK/config.mak"
cp "$MYOS/myos_compat.h" "$WORK/myos_compat.h"
cp "$MYOS/myos_stubs.c" "$WORK/myos_stubs.c"

# Apply ordered myos patches when present.
shopt -s nullglob
for p in "$MYOS"/*.myos.patch; do
  echo "apply $(basename "$p")"
  patch -d "$WORK" -p1 --forward --batch < "$p"
done

# Ensure generated version file exists for offline builds.
if [[ ! -f "$WORK/GIT-VERSION-FILE" ]]; then
  # shellcheck source=ports/git/versions.env
  source "$MYOS/versions.env"
  echo "GIT_VERSION = ${GIT_TAG#v}" >"$WORK/GIT-VERSION-FILE"
fi

echo "git myos tree -> $WORK"
