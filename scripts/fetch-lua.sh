#!/usr/bin/env bash
# Thin wrapper; canonical script is ports/lua/fetch.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/lua/fetch.sh" "$@"
