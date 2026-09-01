#!/usr/bin/env bash
# Thin wrapper; canonical script is toolchain/newlib/tool-wrappers.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/toolchain/newlib/tool-wrappers.sh" "$@"
