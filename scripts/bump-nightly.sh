#!/usr/bin/env bash
# Thin wrapper; canonical script is toolchain/std/bump-nightly.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/toolchain/std/bump-nightly.sh" "$@"
