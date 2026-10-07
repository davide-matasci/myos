#!/usr/bin/env bash
# The fonts for Xft clients: DejaVu Sans Mono (regular, bold, oblique, bold
# oblique), DejaVu Sans (regular: the braille patterns and other symbols
# the Mono faces lack, which fontconfig falls back to) and the DejaVu
# license, from the pinned release tarball into
# target/x11-fonts (installed in /lib/fonts, where the x11-xft package's
# fonts.conf looks). Unpacked with python3's tarfile (.tar.bz2 only, and
# bzip2 is not a build prerequisite).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/x11-fonts/versions.env
source "$HERE/versions.env"

if myos_x11_fonts_is_current; then
  echo "x11-fonts up to date"
  exit 0
fi

CACHE="$ROOT/target/crate-fetch-x11-fonts"
OUT="$ROOT/target/x11-fonts"
tarball="$CACHE/dejavu-fonts-ttf-$DEJAVU_VERSION.tar.bz2"
mkdir -p "$CACHE"
if [[ ! -f "$tarball" ]]; then
  echo "==> fetch DejaVu $DEJAVU_VERSION"
  curl -L --fail --retry 5 --retry-delay 2 -o "$tarball.partial" "$DEJAVU_URL"
  mv "$tarball.partial" "$tarball"
fi
got="$(sha256sum "$tarball" | awk '{print $1}')"
if [[ "$got" != "$DEJAVU_SHA256" ]]; then
  echo "error: DejaVu tarball sha256 $got != pin $DEJAVU_SHA256" >&2
  rm -f "$tarball"
  exit 1
fi
rm -rf "$OUT"
mkdir -p "$OUT"
python3 - "$tarball" "$OUT" "dejavu-fonts-ttf-$DEJAVU_VERSION" <<'PY'
import os, sys, tarfile
tarball, out, top = sys.argv[1:]
wanted = {
    f"{top}/ttf/DejaVuSansMono.ttf": "DejaVuSansMono.ttf",
    f"{top}/ttf/DejaVuSansMono-Bold.ttf": "DejaVuSansMono-Bold.ttf",
    f"{top}/ttf/DejaVuSansMono-Oblique.ttf": "DejaVuSansMono-Oblique.ttf",
    f"{top}/ttf/DejaVuSansMono-BoldOblique.ttf": "DejaVuSansMono-BoldOblique.ttf",
    f"{top}/ttf/DejaVuSans.ttf": "DejaVuSans.ttf",
    f"{top}/LICENSE": "LICENSE.DejaVu",
}
with tarfile.open(tarball, "r:bz2") as t:
    for member in t.getmembers():
        name = wanted.pop(member.name, None)
        if name:
            with t.extractfile(member) as src, open(os.path.join(out, name), "wb") as dst:
                dst.write(src.read())
if wanted:
    sys.exit(f"missing from the tarball: {sorted(wanted)}")
PY
myos_x11_fonts_version_hash >"$MYOS_X11_FONTS_VERSION"
echo "fonts -> $OUT"
