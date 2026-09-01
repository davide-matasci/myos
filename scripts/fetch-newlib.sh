#!/usr/bin/env bash
# Thin wrapper; canonical script is toolchain/newlib/fetch.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/toolchain/newlib/fetch.sh" "$@"
