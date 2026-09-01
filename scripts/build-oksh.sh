#!/usr/bin/env bash
# Thin wrapper; canonical script is ports/oksh/build.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/oksh/build.sh" "$@"
