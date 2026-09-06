#!/usr/bin/env bash
# Thin wrapper; canonical script is ports/vim/fetch.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/vim/fetch.sh" "$@"
