#!/usr/bin/env bash
# Install the in-tree myos libgloss port into a fetched newlib source tree.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NEWLIB_SRC="${NEWLIB_SRC:-$ROOT/target/newlib-src}"
PORT="$ROOT/newlib/libgloss/myos"

if [[ ! -d "$NEWLIB_SRC/newlib" ]]; then
  echo "newlib source missing at $NEWLIB_SRC (run scripts/fetch-newlib.sh)" >&2
  exit 1
fi

echo "==> install myos libgloss port -> $NEWLIB_SRC/libgloss/myos"
rm -rf "$NEWLIB_SRC/libgloss/myos"
mkdir -p "$NEWLIB_SRC/libgloss/myos"
cp -a "$PORT"/. "$NEWLIB_SRC/libgloss/myos/"

patch_config_sub() {
  local f="$NEWLIB_SRC/config.sub"
  if grep -q 'midnightbsd\* | amdhsa\* | unleashed\* | emscripten\* | wasi\* \\' "$f" \
     && ! grep -q 'myos\*' "$f"; then
    sed -i 's/midnightbsd\* | amdhsa\* | unleashed\* | emscripten\* | wasi\* \\/&\n\t     | myos* \\/' "$f"
    echo "patched config.sub for myos"
  fi
}

patch_configure_host() {
  local f="$NEWLIB_SRC/newlib/configure.host"
  if grep -q '\*-\*-myos\*)' "$f"; then
    return
  fi
  sed -i '/^  \*)$/i\
  *-*-myos*)\
\tsyscall_dir=syscalls\
\t;;\
' "$f"
  echo "patched newlib/configure.host for myos"
}

patch_config_sub
patch_configure_host

echo "myos newlib patches applied"
