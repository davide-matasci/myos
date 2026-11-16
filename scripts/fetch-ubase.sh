#!/bin/sh
# Thin wrapper; canonical script is ports/ubase/fetch.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/ports/ubase/fetch.sh" "$@"
