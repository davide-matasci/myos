#!/usr/bin/env bash
# Thin wrapper; canonical script is toolchain/std/package-sysroot.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/toolchain/std/package-sysroot.sh" "$@"
