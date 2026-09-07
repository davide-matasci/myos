#!/usr/bin/env bash
# Thin wrapper; canonical script is ports/zlib/build.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/zlib/build.sh" "$@"
