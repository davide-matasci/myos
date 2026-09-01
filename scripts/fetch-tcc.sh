#!/usr/bin/env bash
# Thin wrapper; canonical script is ports/tcc/fetch.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/tcc/fetch.sh" "$@"
