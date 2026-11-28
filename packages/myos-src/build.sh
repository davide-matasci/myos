#!/bin/sh
# The myos-src package: target/myos-src, the source of this checkout made
# buildable offline.
#
#   the tree        every checked-in file (git ls-files)
#   target/         the crates the tree patches, as their prepare scripts
#                   leave them (acpi, smoltcp), and LibreSSL's source, which
#                   `http` links, as its fetch script leaves it; with the
#                   stamps that tell the scripts there is nothing to fetch.
#                   The sources of the ports a build on myos makes (oksh,
#                   sbase, ubase, dropbear, curl, zlib, Limine's tool), as
#                   their fetch and prepare scripts leave them.
#                   And Rust's library with myos in it (rust-std-patch:
#                   the pinned nightly's rust-src as toolchain/std patches
#                   it, with the crates it vendors), which the std of the
#                   std programs is built from
#   vendor/         every crate of every workspace in the tree (cargo
#                   vendor: the root's, each module's, each program's)
#   Cargo.lock      beside each workspace's manifest: the versions vendor/
#                   holds (the repository keeps no lock file)
#   .cargo/config.toml   the tree's, with crates.io replaced by vendor/
#
# The crates come from crates.io at build time, LibreSSL and the ports from
# their mirrors and the library from the toolchain rustup installed, as in any build of
# the tree; nothing of them is checked in.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
. "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_myos_src_is_current; then
  echo "myos-src up to date"
  exit 0
fi
for tool in git cargo tar curl patch python3 rsync; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "error: $tool not found (README.md)" >&2
    exit 1
  fi
done

OUT="$ROOT/target/myos-src"
rm -rf "$OUT"
mkdir -p "$OUT"
echo "==> myos-src: the tree"
(cd "$ROOT" && git -c safe.directory="$ROOT" ls-files -z | tar --null -T - -cf -) | tar -xf - -C "$OUT"

echo "==> myos-src: the patched crates"
sh "$OUT/modules/acpi/crate/prepare.sh"
sh "$OUT/user/net/smoltcp/prepare.sh"

# newlib as its fetch and patch scripts leave it, without the history.
echo "==> myos-src: newlib's source"
"$ROOT/toolchain/newlib/fetch.sh"
mkdir -p "$OUT/target/newlib-src"
(cd "$ROOT/target/newlib-src" && tar --exclude=./.git -cf - .) | tar -xf - -C "$OUT/target/newlib-src"
sh "$OUT/toolchain/newlib/patch.sh" >/dev/null

echo "==> myos-src: LibreSSL's source"
sh "$OUT/ports/libressl/fetch.sh"

# The ports a build on myos makes (scripts/self-host.sh), each as its
# build script takes it: fetched, and patched where the port has a prepare
# script, without the history, the tests and the manuals.
echo "==> myos-src: the ports' sources"
# port_tree DIR [FILE...]: target/DIR of this checkout into the package's,
# whole or the files named.
port_tree() (
  dir="$1"
  shift
  [ $# -gt 0 ] || set -- .
  mkdir -p "$OUT/target/$dir"
  (cd "$ROOT/target/$dir" && tar --exclude=./.git -cf - "$@") | tar -xf - -C "$OUT/target/$dir"
)
for port in oksh sbase ubase dropbear; do
  "$ROOT/ports/$port/prepare.sh" >/dev/null
  port_tree "$port-myos-build"
done
"$ROOT/ports/curl/fetch.sh" >/dev/null
port_tree curl-src COPYING include lib src .myos-curl-version
"$ROOT/ports/zlib/fetch.sh" >/dev/null
(cd "$ROOT/target/zlib-src" && port_tree zlib-src LICENSE README ./*.c ./*.h)
"$ROOT/ports/limine/fetch.sh" >/dev/null
port_tree limine-src LICENSE limine.c limine-bios-hdd.h .version

echo "==> myos-src: Rust's library"
python3 "$OUT/toolchain/std/patches/wire-myos.py" "$OUT/target/rust-std-patch/library" \
  "$(rustc "+$MYOS_NIGHTLY" --print sysroot)/lib/rustlib/src/rust/library" >/dev/null

# Every workspace: the root's, and each manifest that is one of its own
# (the modules and the programs, which kernel/build.rs builds apart).
echo "==> myos-src: the crates"
cd "$OUT"
set --
for m in $(find kernel modules user -name Cargo.toml | sort); do
  if grep -q '^\[workspace\]' "$m"; then
    set -- "$@" -s "$m"
  fi
done
cargo vendor --versioned-dirs "$@" vendor >"$OUT/.vendor.toml" 2>"$OUT/.vendor.log" \
  || { tail -20 "$OUT/.vendor.log" >&2; exit 1; }
{
  printf '\n# packages/myos-src/build.sh: the crates of the tree are in vendor/.\n'
  sed -n '/^\[source/,$p' "$OUT/.vendor.toml"
} >>"$OUT/.cargo/config.toml"
rm -f "$OUT/.vendor.toml" "$OUT/.vendor.log"
grep -q '^directory = "vendor"$' "$OUT/.cargo/config.toml" \
  || { echo "error: cargo vendor named no vendor directory" >&2; exit 1; }

myos_myos_src_version_hash >"$MYOS_MYOS_SRC_VERSION"
echo "myos-src -> $OUT ($(find "$OUT/vendor" -mindepth 1 -maxdepth 1 | wc -l | tr -d ' ') crates)"
