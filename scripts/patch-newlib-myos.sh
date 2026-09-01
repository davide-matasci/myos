#!/usr/bin/env bash
# Thin wrapper; canonical script is toolchain/newlib/patch.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/toolchain/newlib/patch.sh" "$@"
