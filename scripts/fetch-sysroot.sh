#!/usr/bin/env bash
# Thin wrapper; canonical script is toolchain/std/fetch-sysroot.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/toolchain/std/fetch-sysroot.sh" "$@"
