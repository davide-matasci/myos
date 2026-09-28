#!/bin/bash
# Detached: run riscv64 local CI with gdbstub + attach the t0-step watcher.
cd /root/wt-stall
rm -f /tmp/serial-gdb-r2.log /tmp/gdb-step.out
MYOS_QEMU_GDB=1 MYOS_SERIAL_LOG=/tmp/serial-gdb-r2.log ./scripts/localci-long.sh riscv64 > /tmp/gdb_loop2.log 2>&1 &
LPID=$!
Q=""
for i in $(seq 1 40); do
  Q=$(pgrep -f "^qemu-system-riscv64" | head -1)
  [ -n "$Q" ] && break
  sleep 2
done
echo "loop=$lpid qemu=$Q" > /tmp/gdb-step.state
[ -z "$Q" ] && { echo "no qemu" >> /tmp/gdb-step.state; exit 1; }
sleep 2
gdb-multiarch -q -batch -x /root/wt-stall/scripts/gdb-step-t0.py > /tmp/gdb-step.out 2>&1
echo "gdb exited rc=$?" >> /tmp/gdb-step.state
