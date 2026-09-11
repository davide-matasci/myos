#!/usr/bin/env bash
# Fetch the pinned os-test suite (sortix/os-test) for the myos port and
# assemble the embeddable tree: upstream sources + the myos GNU-make
# harness overlay. Result: target/os-test-embed (read by src/initramfs.rs
# when the port_os_test feature is enabled).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
OSTEST_REV="${OSTEST_REV:-0415c45723798a0ebc150c3990c529a2ff322513}"
OSTEST_SRC="$ROOT/target/os-test-src"
EMBED="$ROOT/target/os-test-embed"

if [[ -d "$OSTEST_SRC/.git" ]]; then
  got="$(git -C "$OSTEST_SRC" rev-parse HEAD)"
  if [[ "$got" != "$OSTEST_REV" ]]; then
    echo "os-test at $got, want $OSTEST_REV; refetching" >&2
    rm -rf "$OSTEST_SRC"
  fi
fi

if [[ ! -d "$OSTEST_SRC" ]]; then
  echo "==> fetch os-test ($OSTEST_REV)"
  "$ROOT/scripts/git-retry.sh" clone https://gitlab.com/sortix/os-test.git "$OSTEST_SRC"
  got="$(git -C "$OSTEST_SRC" rev-parse HEAD)"
  if [[ "$got" != "$OSTEST_REV" ]]; then
    echo "error: os-test HEAD $got != pinned $OSTEST_REV" >&2
    exit 1
  fi
fi

# Assemble embed tree: upstream suites + myos harness overlay (GNU make
# Makefile, myos-run.sh, myos-report.sh).
rm -rf "$EMBED"
mkdir -p "$EMBED"
cp -R "$OSTEST_SRC/." "$EMBED/"
rm -f "$EMBED"/BSDmakefile "$EMBED"/GNUmakefile "$EMBED"/GNUmakefile.os "$EMBED"/BSDmakefile.os
# Upstream ships Makefile as a symlink to the (removed) OS makefiles.
find "$EMBED" -maxdepth 1 -type l -delete
cp -R "$HERE/overlay/." "$EMBED/"
echo "os-test embed tree -> $EMBED"
