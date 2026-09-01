#!/usr/bin/env bash
# Fetch a pinned PCRE2 release for the myos ripgrep port (not vendored in-tree).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=ports/ripgrep/versions.env
source "$ROOT/ports/ripgrep/versions.env"
PCRE2_SRC="$ROOT/target/pcre2-src"
PCRE2_TAG="${PCRE2_TAG:-pcre2-10.45}"

if [[ -d "$PCRE2_SRC/.git" && -f "$PCRE2_SRC/src/pcre2_match.c" ]]; then
  got="$(git -C "$PCRE2_SRC" describe --tags --exact-match 2>/dev/null || true)"
  if [[ "$got" == "$PCRE2_TAG" ]]; then
    echo "pcre2 already present at $PCRE2_SRC ($PCRE2_TAG)"
    exit 0
  fi
  # shallow clones may lack describe; accept matching tag ref
  if git -C "$PCRE2_SRC" rev-parse -q --verify "refs/tags/$PCRE2_TAG" >/dev/null 2>&1 \
    || git -C "$PCRE2_SRC" symbolic-ref -q HEAD >/dev/null 2>&1; then
    echo "pcre2 already present at $PCRE2_SRC"
    exit 0
  fi
fi

if [[ -e "$PCRE2_SRC" ]]; then
  echo "removing incomplete pcre2 tree at $PCRE2_SRC" >&2
  rm -rf "$PCRE2_SRC"
fi

echo "==> fetch pcre2 ($PCRE2_TAG)"
git clone --depth 1 --branch "$PCRE2_TAG" https://github.com/PCRE2Project/pcre2.git "$PCRE2_SRC"
