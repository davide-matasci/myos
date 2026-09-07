#!/usr/bin/env bash
# Host-configure ncurses to generate headers/makefiles, then patch cfg for myos.
#
# Upstream configure cannot --host=*-myos (config.sub) and myos-cc cannot create
# host executables, so we configure for the build machine and cross-compile with
# overridden CC in build.sh.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
"$ROOT/ports/ncurses/fetch.sh"

SRC="$ROOT/target/ncurses-src"
WORK="$ROOT/target/ncurses-myos-build"

# shellcheck source=ports/ncurses/versions.env
source "$HERE/versions.env"
STAMP="$WORK/.myos-prepare-version"
EXPECTED_STAMP="$NCURSES_VERSION:$NCURSES_SHA256:fallbacks=dumb,ansi,vt100,linux"
if [[ -f "$WORK/include/ncurses_cfg.h" && -f "$WORK/ncurses/Makefile" && -f "$STAMP" ]] \
  && [[ "$(cat "$STAMP")" == "$EXPECTED_STAMP" ]]; then
  echo "ncurses prepare tree up to date ($WORK)"
  exit 0
fi

rm -rf "$WORK"
mkdir -p "$WORK"
cd "$WORK"

echo "==> configure ncurses $NCURSES_VERSION (host tools; myos cross later)"
"$SRC/configure" \
  --prefix="$ROOT/target/ncurses-prefix" \
  --without-cxx --without-cxx-binding --without-ada --without-manpages \
  --without-progs --without-tests --without-debug --without-profile \
  --without-gpm --disable-rpath \
  --disable-home-terminfo --disable-db-install --disable-database \
  --enable-termcap --disable-widec --with-fallbacks=dumb,ansi,vt100,linux

CFG="$WORK/include/ncurses_cfg.h"
# Drop host-only features before generating headers. edit_cfg.sh greps for the
# symbol name with spaces — remove the line entirely (not `#define FOO 0`).
python3 - "$CFG" <<'PY'
import re, sys
path = sys.argv[1]
text = open(path).read()
drop = {
    "HAVE_POLL_H", "HAVE_POLL", "HAVE_WORKING_POLL", "HAVE_TERMIO_H",
    "USE_SIGWINCH", "HAVE_SYS_TIMES_H", "HAVE_LOCALE_H", "HAVE_LOCALECONV",
    "HAVE_LANGINFO_CODESET", "HAVE_NANOSLEEP", "HAVE_CLOCK_GETTIME",
    "HAVE_WORKING_VFORK", "HAVE_WORKING_FORK", "HAVE_LINK", "HAVE_SYMLINK",
    "USE_LINKS",
}
out = []
for line in text.splitlines(True):
    m = re.match(r'^\s*(?:/\*\s*)?#\s*(?:define|undef)\s+(\w+)', line)
    if m and m.group(1) in drop:
        continue
    if line.startswith('#define SYSTEM_NAME'):
        out.append('#define SYSTEM_NAME "myos"\n')
        continue
    out.append(line)
open(path, 'w').write(''.join(out))
PY

# Generate include/ headers (curses.h, term.h, …) against the patched cfg.
make -C include

echo "$NCURSES_VERSION:$NCURSES_SHA256:fallbacks=dumb,ansi,vt100,linux" >"$STAMP"
echo "ncurses prepare -> $WORK"
