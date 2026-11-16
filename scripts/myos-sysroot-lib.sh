#!/bin/sh
# Thin wrapper; canonical library is toolchain/std/lib.sh
# (sourced: the script that sources it sets ROOT to the checkout first)
# shellcheck source=toolchain/std/lib.sh
. "${MYOS_ROOT:-${ROOT:?set ROOT to the checkout before sourcing this file}}/toolchain/std/lib.sh"
