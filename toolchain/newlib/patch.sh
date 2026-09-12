#!/usr/bin/env bash
# Install the in-tree myos libgloss port into a fetched newlib source tree.
# NOTE: no `sed -i` here — GNU and BSD sed disagree about -i syntax and
# multiline a/i commands, which broke macOS builds ("extra characters at the
# end of d command"). All in-place edits go through patch_edit (python3).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
NEWLIB_SRC="${NEWLIB_SRC:-$ROOT/target/newlib-src}"
PORT="$ROOT/toolchain/newlib/libgloss/myos"

if [[ ! -d "$NEWLIB_SRC/newlib" ]]; then
  echo "newlib source missing at $NEWLIB_SRC (run toolchain/newlib/fetch.sh)" >&2
  exit 1
fi

echo "==> install myos libgloss port -> $NEWLIB_SRC/libgloss/myos"
rm -rf "$NEWLIB_SRC/libgloss/myos"
mkdir -p "$NEWLIB_SRC/libgloss/myos"
cp -a "$PORT"/. "$NEWLIB_SRC/libgloss/myos/"

patch_edit() {
  # Portable in-place edit. Args: file old new count
  # python3 is a build prerequisite on every host (Linux CI + macOS).
  python3 - "$@" <<'PYEDIT'
import sys
path, old, new, count = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
s = open(path).read()
got = s.count(old)
assert got >= count, f"{path}: expected >= {count} occurrence(s) of {old[:60]!r}, found {got}"
if got > count:
    # Replace only the first `count` occurrences (e.g. configure.host has
    # several '  *)' catch-alls; only the first insertion point is wanted).
    parts = s.split(old)
    s = old.join(parts[:count]) + new + old.join(parts[count:])
    open(path, "w").write(s)
else:
    open(path, "w").write(s.replace(old, new))
PYEDIT
}

patch_config_sub() {
  local f="$NEWLIB_SRC/config.sub"
  if grep -q 'midnightbsd\* | amdhsa\* | unleashed\* | emscripten\* | wasi\* \\' "$f" \
     && ! grep -q 'myos\*' "$f"; then
    patch_edit "$f" \
      'unleashed* | emscripten* | wasi* \' \
      'unleashed* | emscripten* | wasi* \
	     | myos* \' 1
    echo "patched config.sub for myos"
  fi
}

patch_configure_host() {
  local f="$NEWLIB_SRC/newlib/configure.host"
  if grep -q '\*-\*-myos\*)' "$f"; then
    if grep -q 'HAVE_FCNTL' "$f" && ! grep -q 'HAVE_RENAME' "$f"; then
      patch_edit "$f" '-DHAVE_FCNTL' '-DHAVE_FCNTL -DHAVE_RENAME' 1
      echo "patched newlib/configure.host myos: added HAVE_RENAME"
      return
    fi
    if ! grep -q 'HAVE_FCNTL' "$f"; then
      # Insert flags after syscall_dir=syscalls inside the myos arm.
      patch_edit "$f" 'syscall_dir=syscalls' \
        'syscall_dir=syscalls\
	newlib_cflags="${newlib_cflags} -DHAVE_FCNTL -DHAVE_RENAME"' 1
      echo "patched newlib/configure.host myos for HAVE_FCNTL HAVE_RENAME"
    fi
    return
  fi
  patch_edit "$f" \
    '  *)' \
    '  *-*-myos*)\
	syscall_dir=syscalls\
	newlib_cflags="${newlib_cflags} -DHAVE_FCNTL -DHAVE_RENAME"\
	;;\
  *)' 1
  echo "patched newlib/configure.host for myos"
}

patch_config_sub
patch_configure_host

echo "myos newlib patches applied"