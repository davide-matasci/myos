#!/usr/bin/env bash
# The host's side of the kernel's own tests (docs/testing.md): the launcher
# runs `host.sh <what>` for a `HOST tests <what>` line from the guest, in
# the background (cwd = repo root); the guest test decides from what it sees.
#
#   usb-plug      plug the second memory stick (the launcher's `usbhot`
#                 drive) into root port 2 of the xHCI controller, through
#                 the QEMU monitor (the launcher's socket, $MYOS_QEMU_MONITOR)
#   usb-unplug    pull it out again
#   usb-cycle [n] pull it out and push it back in, [n] times (default 20),
#                 as fast as the monitor allows, to race the kernel's USB
#                 detach against I/O the guest keeps in flight. Starts and
#                 ends plugged (the caller plugs it first).
#
# `device_del` of the usb-storage also drops its `usbhot` drive backend, so a
# re-plug must re-create the drive (`drive_add`) before `device_add`; the
# duplicate-id error on the first plug is harmless and ignored.
set -u

IMG="target/usb-hot.img"
DRV="drive_add 0 if=none,id=usbhot,format=raw,file=${IMG}"
ADD="device_add usb-storage,bus=xhci.0,port=2,drive=usbhot,id=usbhot"
DEL="device_del usbhot"

# Send one or more monitor commands over a single connection; print any
# QEMU error so a flaky command is not mistaken for a kernel fault.
mon() {
  python3 - "${MYOS_QEMU_MONITOR:?no QEMU monitor}" "$@" <<'PY'
import socket, sys, time
sock = sys.argv[1]
cmds = sys.argv[2:]
s = socket.socket(socket.AF_UNIX)
s.connect(sock)
s.settimeout(5)
s.recv(4096)  # banner and prompt
for cmd in cmds:
    s.sendall(f"{cmd}\n".encode())
    time.sleep(0.12)
    try:
        reply = s.recv(65536).decode(errors="replace")
    except Exception:
        reply = ""
    for line in reply.splitlines():
        line = line.strip()
        if line.startswith("Error:"):
            print(f"boot test: monitor error: {line}", file=sys.stderr)
s.close()
PY
}

plug() { mon "$DRV" "$ADD"; echo "boot test: monitor: usb plug" >&2; }
unplug() { mon "$DEL"; echo "boot test: monitor: usb unplug" >&2; }

# Rapid unplug/replug. Each replug re-creates the drive first. Ends plugged.
cycle() {
  local n="${1:-20}" seq=() i
  for ((i = 0; i < n; i++)); do
    seq+=("$DEL" "$DRV" "$ADD")
  done
  mon "${seq[@]}"
  echo "boot test: monitor: usb-cycle $n" >&2
}

case "${1:-}" in
  usb-plug) plug ;;
  usb-unplug) unplug ;;
  usb-cycle) cycle "${2:-20}" ;;
  *) echo "host.sh: unknown request: $*" >&2; exit 2 ;;
esac
