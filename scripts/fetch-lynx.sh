#!/usr/bin/env bash
# Thin wrapper; canonical script is ports/lynx/fetch.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/lynx/fetch.sh" "$@"
