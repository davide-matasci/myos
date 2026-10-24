#!/usr/bin/env bash
# Stage the rust app for the three arches, target/rust-<arch>: the myos std
# of the arch's target from the sysroot (its rlibs, without the .rmeta
# copies, and its spec), and the `rust` command with the arch's and the
# pin's values filled in. Nothing is compiled.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=toolchain/std/lib.sh
source "$ROOT/toolchain/std/lib.sh"
# shellcheck source=packages/rust/versions.env
source "$HERE/versions.env"

if myos_rust_is_current; then
  echo "rust up to date"
  exit 0
fi

# The std below is the pinned nightly's: the toolchain the app fetches must
# be the same one.
if [[ "nightly-$RUST_NIGHTLY" != "$MYOS_NIGHTLY" ]]; then
  echo "packages/rust/versions.env pins nightly-$RUST_NIGHTLY, rust-toolchain.toml $MYOS_NIGHTLY:" >&2
  echo "run toolchain/std/bump-nightly.sh --write" >&2
  exit 1
fi
"$ROOT/toolchain/std/fetch-sysroot.sh"

for arch in x86_64 aarch64 riscv64; do
  target="$arch-unknown-myos"
  case $arch in
    x86_64) host=x86_64-unknown-linux-musl ldso=ld-musl-x86_64.so.1 ;;
    aarch64) host=aarch64-unknown-linux-musl ldso=ld-musl-aarch64.so.1 ;;
    riscv64) host=riscv64gc-unknown-linux-musl ldso=ld-musl-riscv64.so.1 ;;
  esac
  rustc_sum="RUST_RUSTC_$arch" cargo_sum="RUST_CARGO_$arch" std_sum="RUST_STD_$arch"
  app="$ROOT/target/rust-$arch"
  rm -rf "$app"
  mkdir -p "$app/lib/rust/$target/lib" "$app/bin/custom"
  cp "$MYOS_SYSROOT/lib/rustlib/$target/lib/"*.rlib "$app/lib/rust/$target/lib/"
  cp "$ROOT/targets/$target.json" "$app/lib/rust/"
  sed -e "s|@NIGHTLY@|$RUST_NIGHTLY|" -e "s|@HOST@|$host|" -e "s|@TARGET@|$target|g" \
    -e "s|@LDSO@|$ldso|" -e "s|@RUSTC@|${!rustc_sum}|" -e "s|@CARGO@|${!cargo_sum}|" \
    -e "s|@STD@|${!std_sum}|" "$HERE/rust" >"$app/bin/custom/rust"
  chmod 755 "$app/bin/custom/rust"
  echo "rust ($arch): $(du -sh "$app" | cut -f1)"
done
echo "$(myos_rust_version_hash)" >"$MYOS_RUST_VERSION"
