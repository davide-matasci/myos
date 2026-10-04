#!/usr/bin/env bash
# The host's side of the kernel's own tests (docs/testing.md): the launcher
# runs `host.sh <what>` for a `HOST tests <what>` line from the guest, in
# the background; the guest test decides from what it sees.
#
#   usb-plug      plug the second memory stick (the launcher's `usbhot`
#                 drive) into root port 2 of the xHCI controller, through
#                 the QEMU monitor (the launcher's socket, $MYOS_QEMU_MONITOR)
#   usb-unplug    pull it out again
set -u

monitor() {
  python3 - "${MYOS_QEMU_MONITOR:?no QEMU monitor}" "$1" <<'PY'
import socket, sys, time
s = socket.socket(socket.AF_UNIX)
s.connect(sys.argv[1])
s.settimeout(5)
s.recv(4096)  # the banner and prompt
s.sendall(f"{sys.argv[2]}\n".encode())
time.sleep(1)
s.close()
PY
  echo "boot test: monitor: $1" >&2
}

case "${1:-}" in
  usb-plug) monitor "device_add usb-storage,bus=xhci.0,port=2,drive=usbhot,id=usbhot" ;;
  usb-unplug) monitor "device_del usbhot" ;;
  *) echo "host.sh: unknown request: $*" >&2; exit 2 ;;
esac
