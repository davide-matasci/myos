#!/usr/bin/env bash
# Run riscv64 local CI N times, log each result. For fix/riscv64-ssh-stall stability proof.
set -u
cd /root/wt-stall
N=${1:-10}
SUMMARY=/tmp/stall_loop_summary.txt
: > "$SUMMARY"
for i in $(seq 1 "$N"); do
    echo "=== RUN $i $(date -u +%H:%M:%S) ===" >> "$SUMMARY"
    pkill -9 -f qemu-system 2>/dev/null
    sleep 1
    MYOS_SERIAL_LOG=/tmp/serial-riscv64-r$i.log ./scripts/localci-long.sh riscv64 >> /tmp/stall_loop_run$i.log 2>&1
    rc=$?
    echo "RUN $i exit=$rc" >> "$SUMMARY"
done
echo "ALL DONE $(date -u)" >> "$SUMMARY"
