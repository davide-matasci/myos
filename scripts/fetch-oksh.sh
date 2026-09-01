#!/usr/bin/env bash
# Thin wrapper; canonical script is ports/oksh/fetch.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/oksh/fetch.sh" "$@"
