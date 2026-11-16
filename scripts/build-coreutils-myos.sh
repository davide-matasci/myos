#!/bin/sh
# Thin wrapper; canonical script is packages/coreutils/build.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/packages/coreutils/build.sh" "$@"
