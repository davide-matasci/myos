#!/usr/bin/env bash
# Thin wrapper; canonical script is toolchain/std/check-wire.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/toolchain/std/check-wire.sh" "$@"
