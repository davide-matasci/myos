#!/usr/bin/env bash
# Thin wrapper; canonical script is toolchain/newlib/build-libgloss.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/toolchain/newlib/build-libgloss.sh" "$@"
