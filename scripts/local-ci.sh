#!/usr/bin/env bash
# A local boot test (docs/testing.md) with the settings a loaded host needs:
# 1. OOM: some sandboxes set oom_score_adj=1000 on children, so a TCG QEMU
#    with a few GiB gets SIGKILLed under memory pressure. Lower our own
#    score (QEMU inherits it).
# 2. Stale state: a leftover QEMU holds target/bios.img or the host ports
#    the tests forward (:2323, :2222, :8765). Clean up and warn.
# 3. MTTCG starvation: four vCPU threads on a loaded 4-core host stall the
#    boot with no serial output. Single-threaded TCG by default
#    (MYOS_TCG_SINGLE).
#
# Usage: scripts/local-ci.sh [bios|uefi|aarch64|riscv64] [mini|full]   (default bios mini)
set -euo pipefail
ARCH="${1:-bios}"
LIST="${2:-mini}"

if [ -w /proc/self/oom_score_adj ]; then
    echo -500 > /proc/self/oom_score_adj 2>/dev/null || true
fi

pkill -9 -f qemu-system 2>/dev/null || true
sleep 1

for port in 2323 2222 8765; do
    if ss -tln 2>/dev/null | grep -q ":$port "; then
        echo "WARNING: host :$port busy -> the test that needs it will fail" >&2
    fi
done

cd "$(dirname "$0")/.."
export MYOS_TCG_SINGLE="${MYOS_TCG_SINGLE:-1}"

LOG="${TMPDIR:-/tmp}/localci-$ARCH-$LIST-$(date +%s).log"
STATUS=0
nice -n 5 cargo run --quiet -- "$ARCH" "test-$LIST" >"$LOG" 2>&1 || STATUS=$?
tail -5 "$LOG"
echo "log: $LOG"
exit $STATUS
