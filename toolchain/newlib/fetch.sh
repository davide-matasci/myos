#!/usr/bin/env bash
# Fetch a pinned newlib release for the myos port.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
NEWLIB_SRC="$ROOT/target/newlib-src"
NEWLIB_TAG="${NEWLIB_TAG:-newlib-4.4.0}"
# The commit the tag names: what a clone must be, wherever it came from.
NEWLIB_REV="${NEWLIB_REV:-26f7004bf73c421c3fd5e5a6ccf470d05337b435}"
# sourceware.org, and its mirror on GitHub: sourceware answers some
# networks (GitHub's runners, at times) with 403.
NEWLIB_URLS=(https://sourceware.org/git/newlib-cygwin.git https://github.com/bminor/newlib.git)

if [[ -d "$NEWLIB_SRC/newlib" && -f "$NEWLIB_SRC/config.sub" ]]; then
  echo "newlib already present at $NEWLIB_SRC"
  exit 0
fi

if [[ -e "$NEWLIB_SRC" ]]; then
  echo "removing incomplete newlib tree at $NEWLIB_SRC" >&2
  rm -rf "$NEWLIB_SRC"
fi

echo "==> fetch newlib ($NEWLIB_TAG)"
for url in "${NEWLIB_URLS[@]}"; do
  if "$ROOT/scripts/git-retry.sh" clone --depth 1 --branch "$NEWLIB_TAG" "$url" "$NEWLIB_SRC"; then
    got="$(git -C "$NEWLIB_SRC" rev-parse HEAD)"
    if [[ "$got" != "$NEWLIB_REV" ]]; then
      echo "error: newlib $NEWLIB_TAG from $url is $got, want $NEWLIB_REV" >&2
      exit 1
    fi
    exit 0
  fi
  echo "warning: no newlib from $url" >&2
  rm -rf "$NEWLIB_SRC"
done
echo "error: newlib $NEWLIB_TAG could not be fetched" >&2
exit 1
