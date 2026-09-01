#!/usr/bin/env bash
# Thin wrapper; canonical library is toolchain/std/lib.sh
# shellcheck source=toolchain/std/lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/toolchain/std/lib.sh"
