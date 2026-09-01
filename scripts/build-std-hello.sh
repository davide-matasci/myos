#!/usr/bin/env bash
# Thin wrapper; canonical script is toolchain/std/build-std-hello.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/toolchain/std/build-std-hello.sh" "$@"
