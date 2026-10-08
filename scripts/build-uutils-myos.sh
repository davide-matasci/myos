#!/usr/bin/env bash
# Thin wrapper; canonical script is packages/coreutils/build-uutils.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/packages/coreutils/build-uutils.sh" "$@"
