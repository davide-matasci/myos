#!/usr/bin/env bash
# Thin wrapper; canonical script is toolchain/std/prepare.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/toolchain/std/prepare.sh" "$@"
