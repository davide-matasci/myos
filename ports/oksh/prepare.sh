#!/usr/bin/env bash
# Prepare a myos build tree from fetched upstream oksh (pconfig.h + patches).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
"$ROOT/ports/oksh/fetch.sh"

OKSH="$ROOT/target/oksh-src"
WORK="$ROOT/target/oksh-myos-build"
MYOS="$ROOT/ports/oksh"

rm -rf "$WORK"
mkdir -p "$WORK"

rsync -a \
  --exclude='.git' \
  --exclude='CVS' \
  --exclude='*.1' \
  "$OKSH/" "$WORK/"

cp "$MYOS/pconfig.h" "$WORK/pconfig.h"
# Replace ulimit rather than a 200-line reverse patch (needs getrlimit).
cp "$MYOS/c_ulimit.c" "$WORK/c_ulimit.c"
# tty.c stays pristine stock: myos implements /dev/tty, F_DUPFD/F_DUPFD_CLOEXEC
# and libgloss termios, so oksh's real tty_init (dup console via F_DUPFD into
# tty_fd at FDBASE) works unchanged — no replacement shim needed.

patch_copy() {
  local base="$1"
  patch -d "$WORK" -p0 --forward --batch < "$MYOS/${base}.myos.patch"
}

patch_copy main
patch_copy jobs
patch_copy emacs
patch_copy trap
patch_copy io
patch_copy shf
patch_copy c_sh
patch_copy lex
patch_copy c_ksh
patch_copy mail

echo "oksh myos tree -> $WORK"
