#!/bin/sh
# Thin wrapper; canonical script is ports/sbase/fetch.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/sbase/fetch.sh" "$@"
