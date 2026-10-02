#!/usr/bin/env bash
# Thin wrapper; canonical script is user/std/build.sh
exec "$(cd "$(dirname "$0")/.." && pwd)/user/std/build.sh" "$@"
