#!/usr/bin/env bash
# Thin wrapper; canonical script is toolchain/std/build-sysroot.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/toolchain/std/build-sysroot.sh" "$@"
