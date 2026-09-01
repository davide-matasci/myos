#!/usr/bin/env bash
# Thin wrapper; canonical script is toolchain/newlib/build.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/toolchain/newlib/build.sh" "$@"
