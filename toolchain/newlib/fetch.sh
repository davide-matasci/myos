#!/usr/bin/env bash
# Fetch a pinned newlib release for the myos port.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
NEWLIB_SRC="$ROOT/target/newlib-src"
NEWLIB_TAG="${NEWLIB_TAG:-newlib-4.4.0}"
NEWLIB_URL="${NEWLIB_URL:-https://sourceware.org/git/newlib-cygwin.git}"

if [[ -d "$NEWLIB_SRC/newlib" && -f "$NEWLIB_SRC/config.sub" ]]; then
  echo "newlib already present at $NEWLIB_SRC"
  exit 0
fi

if [[ -e "$NEWLIB_SRC" ]]; then
  echo "removing incomplete newlib tree at $NEWLIB_SRC" >&2
  rm -rf "$NEWLIB_SRC"
fi

clone_newlib() {
  git clone --depth 1 --branch "$NEWLIB_TAG" "$NEWLIB_URL" "$NEWLIB_SRC"
}

echo "==> fetch newlib ($NEWLIB_TAG)"
attempt=1
max_attempts=5
delay=30
while true; do
  if clone_newlib; then
    exit 0
  fi
  rm -rf "$NEWLIB_SRC"
  if ((attempt >= max_attempts)); then
    echo "error: newlib fetch failed after ${max_attempts} attempts" >&2
    exit 1
  fi
  echo "newlib fetch failed (attempt ${attempt}/${max_attempts}), retrying in ${delay}s..." >&2
  sleep "$delay"
  attempt=$((attempt + 1))
  delay=$((delay * 2))
done
