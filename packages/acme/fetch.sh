#!/usr/bin/env bash
# Fetch the pinned acme.sh release (a git tag, checked against its commit)
# into target/acme-src.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=packages/acme/versions.env
source "$HERE/versions.env"
SRC="$ROOT/target/acme-src"

if [[ -d "$SRC/.git" && "$(git -C "$SRC" rev-parse HEAD)" == "$ACME_REV" ]]; then
  echo "acme.sh sources up to date ($ACME_VERSION)"
  exit 0
fi
rm -rf "$SRC"
echo "==> fetch acme.sh $ACME_VERSION"
"$ROOT/scripts/git-retry.sh" clone --depth 1 --branch "$ACME_VERSION" \
  https://github.com/acmesh-official/acme.sh "$SRC"
got="$(git -C "$SRC" rev-parse HEAD)"
if [[ "$got" != "$ACME_REV" ]]; then
  echo "acme.sh $ACME_VERSION is $got, not the pinned $ACME_REV" >&2
  exit 1
fi
echo "acme.sh fetch ok ($ACME_VERSION)"
