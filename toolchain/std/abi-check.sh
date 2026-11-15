#!/usr/bin/env bash
# Check the libc crate's myos module (patches/libc-myos.patch) against
# mlibc's headers, for the three arches: every constant, type size and
# field offset the crate declares and the headers have (abi-check.py).
# Run it after a change to the patch, to mlibc's ABI headers
# (toolchain/mlibc/sysdeps/myos/include/abi-bits) or a bump of either.
# Needs the sysroot (its patched library tree) and mlibc's headers.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

"$ROOT/toolchain/std/fetch-sysroot.sh"
"$ROOT/toolchain/mlibc/build.sh"
myos_ensure_llvm_bin
myos_export_toolchain_env
rc=0
for arch in ${MYOS_ABI_ARCHES:-x86_64 aarch64 riscv64}; do
  MYOS_ABI_CFLAGS="$(myos_mlibc_cflags "$arch")" python3 "$HERE/abi-check.py" "$arch" || rc=1
done
exit $rc
