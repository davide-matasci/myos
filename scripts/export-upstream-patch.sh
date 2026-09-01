#!/usr/bin/env bash
# Thin wrapper; canonical script is toolchain/std/export-upstream-patch.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/toolchain/std/export-upstream-patch.sh" "$@"
