#!/usr/bin/env bash
# Fetch the pinned ATK and GTK sources into target/{atk,gtk}-src, GTK
# with its patches (gtk-*.myos.patch) applied.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=packages/gtk3/versions.env
source "$HERE/versions.env"

# fetch NAME VERSION URL SHA256 TAR-FLAG [PATCH...]: target/NAME-src from
# the tarball, with the PATCHes applied.
fetch() {
  local name="$1" version="$2" url="$3" sha="$4" flag="$5"
  shift 5
  local src="$ROOT/target/$name-src" stamp="$ROOT/target/.$name-src-version"
  local want="$version" p
  for p in "$@"; do want+=" $(sha256sum <"$p" | cut -c1-16)"; done
  if [[ -f "$stamp" && "$(cat "$stamp")" == "$want" && -d "$src" ]]; then
    echo "$name sources up to date ($version)"
    return
  fi
  rm -rf "$src"
  mkdir -p "$ROOT/target"
  local tmp
  tmp="$(mktemp -d)"
  echo "fetch $name $version"
  curl -fsSL --retry 5 --retry-delay 2 "$url" -o "$tmp/src.tar"
  echo "$sha  $tmp/src.tar" | sha256sum -c -
  mkdir -p "$tmp/extract"
  if [[ "$flag" == J ]] && ! command -v xz >/dev/null 2>&1; then
    python3 -c 'import lzma, sys; sys.stdout.buffer.write(lzma.decompress(sys.stdin.buffer.read()))' \
      <"$tmp/src.tar" | tar -x -C "$tmp/extract"
  else
    tar -x"$flag"f "$tmp/src.tar" -C "$tmp/extract"
  fi
  mv "$tmp/extract/$name-$version" "$src"
  rm -rf "$tmp"
  for p in "$@"; do
    patch -d "$src" -p1 -s <"$p"
  done
  echo "$want" >"$stamp"
}

fetch atk "$ATK_VERSION" "$ATK_URL" "$ATK_SHA256" J
fetch gtk "$GTK_VERSION" "$GTK_URL" "$GTK_SHA256" J "$HERE"/gtk-*.myos.patch
echo "gtk3 fetch ok ($GTK_VERSION)"
