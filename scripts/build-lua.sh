#!/usr/bin/env bash
# Thin wrapper; canonical script is ports/lua/build.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/lua/build.sh" "$@"
