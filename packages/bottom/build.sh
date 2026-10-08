#!/usr/bin/env bash
# Cross-build bottom (btm) for the three arches on the myos std sysroot:
# the bottom, crossterm, sysinfo, dirs-sys and parking_lot_core crates from
# crates.io with their myos patches (*.myos.patch, README.md), every other
# dependency as bottom's Cargo.lock pins it, and the myos libc, errno and
# rustix crates the Rust ports share (packages/coreutils/prepare.sh). Then
# btm_smoke, the boot test's pty driver (C).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/bottom/versions.env
source "$HERE/versions.env"

if myos_bottom_is_current; then
  echo "bottom up to date"
  exit 0
fi

NIGHTLY="${MYOS_NIGHTLY:-nightly-2026-07-26}"
RUST_LLD_BIN="$(rustc "+$NIGHTLY" --print sysroot)/lib/rustlib/$(rustc "+$NIGHTLY" -vV | awk '/host:/{print $2}')/bin"

"$HERE/fetch.sh"
"$ROOT/toolchain/std/fetch-sysroot.sh"
"$ROOT/toolchain/newlib/build.sh"
"$ROOT/packages/coreutils/prepare.sh"
export PATH="$ROOT/target/newlib-bin:$RUST_LLD_BIN:$PATH"

WORK="$ROOT/target/bottom-build"
CRATES="$WORK/crates"
rm -rf "$CRATES"
mkdir -p "$CRATES"

# unpack NAME VERSION: the crate, with its myos patch applied.
unpack() {
  tar -xzf "$ROOT/target/bottom-crates/$1-$2.crate" -C "$CRATES"
  patch -d "$CRATES/$1-$2" -p1 --forward --batch -s < "$HERE/$1.myos.patch"
}
unpack bottom "$BOTTOM_VERSION"
unpack crossterm "$CROSSTERM_VERSION"
unpack sysinfo "$SYSINFO_VERSION"
unpack dirs-sys "$DIRS_SYS_VERSION"
unpack parking_lot_core "$PARKING_LOT_CORE_VERSION"

SRC="$CRATES/bottom-$BOTTOM_VERSION"
P="$ROOT/target/patched-crates"
cat >>"$SRC/Cargo.toml" <<EOF

# --- myos: packages/bottom/build.sh ---
# Its own workspace: target/ is inside myos's.
[workspace]

[patch.crates-io]
crossterm = { path = "$CRATES/crossterm-$CROSSTERM_VERSION" }
sysinfo = { path = "$CRATES/sysinfo-$SYSINFO_VERSION" }
dirs-sys = { path = "$CRATES/dirs-sys-$DIRS_SYS_VERSION" }
parking_lot_core = { path = "$CRATES/parking_lot_core-$PARKING_LOT_CORE_VERSION" }
errno = { path = "$P/errno-0.3.14" }
libc = { path = "$P/libc-0.2.189" }
rustix = { path = "$P/rustix-1.1.4" }
getrandom = { path = "$P/getrandom-0.2.17" }
EOF

# The myos targets, as the other Rust ports build them (ports/ripgrep): the
# std sysroot, and newlib + libgloss behind the libc crate (getpwuid).
{
  cat <<EOF

[unstable]
json-target-spec = true

[env]
MYOS_SYSROOT = { value = "$ROOT/target/myos-sysroot", relative = false }
RUSTC_BOOTSTRAP = "1"

[build]
rustc = "$ROOT/scripts/myos-rustc-cross.sh"
EOF
  for arch in x86_64 aarch64 riscv64; do
    cat <<EOF

[target.$arch-unknown-myos]
linker = "rust-lld"
rustflags = [
  "-C", "panic=abort",
  "--cfg=rustix_use_libc",
  "-C", "link-arg=--allow-multiple-definition",
  "-C", "link-arg=-L$ROOT/target/newlib-$arch/$arch-unknown-myos/lib",
  "-C", "link-arg=-lc",
  "-C", "link-arg=-lgloss",
]
EOF
  done
} >>"$SRC/.cargo/config.toml"

# Cargo fingerprints the sources and flags it builds with, not what it builds
# against outside them: the std sysroot, newlib (behind the libc crate) and the
# target spec. A target dir left by a build against other ones (an older local
# build, or the one CI's cache restores into target/) holds rlibs rustc refuses
# next to the new std ("can't find crate for `bitflags`", "found possibly newer
# version of crate `core`"), so a triple's target dir starts over when any of
# them changed.
toolchain="$(cat "$MYOS_SYSROOT_VERSION") $(cat "$MYOS_NEWLIB_VERSION") $NIGHTLY"

for arch in x86_64 aarch64 riscv64; do
  triple="$arch-unknown-myos"
  tdir="$WORK/target-$triple"
  stamp="$toolchain $(sha256sum <"$ROOT/targets/$triple.json" | cut -d' ' -f1)"
  [[ "$(cat "$tdir/.myos-toolchain" 2>/dev/null)" == "$stamp" ]] || rm -rf "$tdir"
  echo "==> bottom ($triple)"
  (
    cd "$SRC"
    unset RUSTC
    # No LTO (bottom's release profile has it): the prebuilt std carries no
    # bitcode.
    CARGO_PROFILE_RELEASE_LTO=false cargo "+$NIGHTLY" build --release --no-default-features \
      --target "$ROOT/targets/$triple.json" --bin btm \
      --target-dir "$tdir"
  )
  echo "$stamp" >"$tdir/.myos-toolchain"
  cp "$tdir/$triple/release/btm" "$ROOT/target/btm-$triple"
  echo "btm -> target/btm-$triple ($(du -h "$ROOT/target/btm-$triple" | cut -f1))"

  cc="$WORK/$arch-cc"
  myos_write_cross_cc "$arch" "$cc"
  "$cc" -O2 -Wall -Wextra "$HERE/btm_smoke.c" -o "$ROOT/target/btm-smoke-$arch-unknown-none"
done

myos_bottom_version_hash >"$MYOS_BOTTOM_VERSION"
echo "bottom -> target/btm-<arch>-unknown-myos, target/btm-smoke-<arch>-unknown-none"
