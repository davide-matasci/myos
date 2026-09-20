#!/usr/bin/env bash
# Fetch + host-prebuild os-test (embed tree + boot-CI curated ELFs).
# Same ports-base contract as curl/tcc: stamp + early-exit when current;
# CI restores via ci-registry.sh; local: ./ports/os-test/build.sh
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=ports/os-test/versions.env
source "$HERE/versions.env"

if myos_os_test_is_current; then
  echo "os-test embed + prebuilts up to date"
  exit 0
fi

"$HERE/fetch.sh"
# newlib sysroots required for host-prebuild (same as c-hello / tcc).
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"
"$HERE/prebuild-basic-smoke.sh"

# Require all three arches (CI / full local); fail clearly if a sysroot was
# missing so prebuild skipped an arch.
missing=0
for arch in x86_64 aarch64 riscv64; do
  marker="$ROOT/target/os-test-prebuilt/${arch}/basic/arpa_inet/htons"
  if [[ ! -f "$marker" ]]; then
    echo "error: os-test prebuilt missing for ${arch} at ${marker}" >&2
    missing=1
  fi
  nb="$ROOT/target/os-test-prebuilt/${arch}/limits/CHAR_BIT"
  if [[ ! -f "$nb" ]]; then
    echo "error: os-test nonbasic prebuilt missing for ${arch} at ${nb}" >&2
    missing=1
  fi
done
probe="$ROOT/target/os-test-embed/basic/ctype/isalnum.c"
if [[ ! -f "$probe" ]]; then
  echo "error: os-test embed incomplete (missing ${probe})" >&2
  missing=1
fi
if ((missing != 0)); then
  exit 1
fi

echo "$(myos_os_test_version_hash)" >"$MYOS_OS_TEST_VERSION"
echo "os-test build ok (rev ${OSTEST_REV})"
