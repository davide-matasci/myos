#!/usr/bin/env bash
# Thin wrapper; canonical script is ports/git/fetch.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/git/fetch.sh" "$@"
