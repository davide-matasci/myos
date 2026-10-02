#!/usr/bin/env bash
# ISO job: extract ci-build.tar then cargo build + iso, with the Linux layer
# (`--features linux_compat`: the `linux` module loaded at boot, the musl
# tests and get-alpine, whose files the build job packs). Must not rebuild
# ports.
set -euo pipefail
chmod +x scripts/*.sh ports/*/*.sh toolchain/*/*.sh 2>/dev/null || true
if [[ ! -f ci-build.tar ]]; then
  echo "::error::ci-build.tar missing after download; build job must upload it"
  exit 1
fi
echo "==> extracting ci-build.tar"
tar -xf ci-build.tar
test -x target/debug/myos && echo "host myos present; reusing build outputs"
# newlib wrappers needed for mbedtls during cargo build of myos host tool.
./toolchain/newlib/tool-wrappers.sh
export PATH="$PWD/target/newlib-bin:$PATH"
cargo clean -p myos
cargo build --features linux_compat
cargo run --features linux_compat -- iso
test -f target/myos-x86_64.iso
