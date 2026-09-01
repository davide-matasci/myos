#!/usr/bin/env bash
# Thin wrapper; canonical script is toolchain/std/install-sysroot.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/toolchain/std/install-sysroot.sh" "$@"
