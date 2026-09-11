#!/usr/bin/env bash
# Prepare a myos build tree from the fetched GNU make tarball.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=ports/make/versions.env
source "$HERE/versions.env"
"$HERE/fetch.sh"

SRCROOT="$ROOT/target/make-src"
WORK="$ROOT/target/make-myos-build"

rm -rf "$SRCROOT" "$WORK"
mkdir -p "$WORK" "$SRCROOT"
tar -xzf "$ROOT/target/make-${MAKE_VERSION}.tar.gz" -C "$SRCROOT" --strip-components=1

# Copy the src/ tree we build from (POSIX path only), plus headers.
for f in "$SRCROOT"/src/*.c "$SRCROOT"/src/*.h; do
  base="$(basename "$f")"
  case "$base" in
    amiga.*|vms*|config.h.W32|config.ami|configh.dos) continue ;;
  esac
  cp "$f" "$WORK/"
done

# config.h lives in the port dir; makeint.h includes <config.h> via -I.
cp "$HERE/config.h" "$WORK/config.h"
cp "$HERE/myos_compat.h" "$WORK/myos_compat.h"

# myos crt0 calls main(argc, argv) only: rdx (envp) is garbage on entry,
 # and main.c walks envp[] to import environment variables. Declare envp
 # from the real environ unconditionally (upstream guards it under MK_OS_ZOS).
 python3 - "$WORK/main.c" <<'PYEOF'
import sys
p=sys.argv[1]
s=open(p).read()
old="#ifdef MK_OS_ZOS\n  char **envp = environ;\n#endif\n"
assert old in s, "envp guard not found"
s=s.replace(old,"  char **envp = environ;\n",1)
# main's third parameter (envp) would now collide with the local above.
old2="main (int argc, char **argv, char **envp)"
assert old2 in s, "main signature not found"
s=s.replace(old2,"main (int argc, char **argv, char **envp_unused)",1)
open(p,'w').write(s)
print("main.c envp patched")
PYEOF

# newlib's <glob.h> is a bare subset (no GLOB_NOMATCH), so bundle make's
# own gnulib glob/fnmatch: -I$WORK precedes -isystem newlib include.
cp "$SRCROOT/lib/glob.c" "$SRCROOT/lib/fnmatch.c" "$WORK/"
cp "$SRCROOT/lib/glob.in.h" "$WORK/glob.h"
cp "$SRCROOT/lib/fnmatch.in.h" "$WORK/fnmatch.h"

# gnulib headers the src/ tree includes (intprops used by ar.c etc.).
for h in intprops.h intprops-internal.h warn-on-use.h filename.h; do
  cp "$SRCROOT/lib/$h" "$WORK/" 2>/dev/null || true
done

echo "make build tree ready at $WORK"
