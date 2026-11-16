#!/bin/sh
# Thin wrapper; canonical script is ports/zlib/fetch.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/zlib/fetch.sh" "$@"
