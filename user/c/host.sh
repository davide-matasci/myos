#!/usr/bin/env bash
# The host's side of the C smokes' tests (docs/testing.md): the launcher runs
# `host.sh <what> <args>` for a `HOST c-smokes <what> <args>` line from the
# guest, in the background; the guest test decides from what it sees.
#
#   tcp-ping PORT TOKEN  the listen/accept smoke's peer: connect to the
#                   guest's listener through QEMU's port forward, send
#                   "ping", expect "pong" and then the smoke's numbered
#                   lines up to its close; connect again to say "good" if
#                   every byte arrived ("bad <bytes>" if not). Retried until
#                   the listener is up or `tcp-ping-stop` with the same
#                   TOKEN (the guest's name for this run of the smoke)
#                   says the smoke is gone; the guest test bounds its own
#                   wait. Then 40 connections that close without a word,
#                   the close 0 to 20 ms after the connect (the FIN with or
#                   right after the handshake's last ACK), and one that
#                   says "done": the smoke's accepts must all end.
#   tcp-ping-stop PORT TOKEN  the smoke `tcp-ping PORT TOKEN` was the peer
#                   of failed: its peer stops retrying (it kept on for its
#                   three minutes once, onto the next listen test's smoke,
#                   whose own peer then found a connection taken).
#   sendkey KEYS... type each of KEYS (QEMU's names, `shift-a`), in order,
#                   on the guest's keyboard through the QEMU monitor (the
#                   launcher's socket, $MYOS_QEMU_MONITOR). One request per
#                   key would not keep the order: requests run in parallel.
#                   KEY@MS holds the key down MS milliseconds.
#   keyburst N KEY  type KEY N times, 10 ms apart: a burst of interrupts
#                   for the keyboard's readers to drain together.
#   mouse [ps2|virtio]  move the guest's usb-mouse 10 right and 5 up,
#                   then click its left button (mouse_smoke.c), through the
#                   monitor; with `ps2` or `virtio`, QEMU's PS/2 or virtio
#                   mouse instead (the monitor's `mouse_set` makes it the
#                   one that moves, and gives the usb-mouse back after).
#   usb-kbd plug|unplug  plug a usb-kbd into root port 4 of the xHCI
#                   controller (or pull it out), through the monitor. While
#                   it is in, `sendkey` types on it: QEMU gives the keys to
#                   the keyboard added last, then back to the PS/2 or virtio
#                   one.
set -u

# The smoke's bulk: BULK_LINES lines "%06d\n" (tcp_listen_smoke.c).
BULK_LINES=40000

# The file `tcp-ping-stop PORT TOKEN` leaves for `tcp-ping PORT TOKEN`,
# next to this boot's QEMU monitor socket (one launcher, one boot at a
# time); the latter removes it when it ends.
stop_flag() {
  echo "${MYOS_QEMU_MONITOR:-${TMPDIR:-/tmp}/myos-monitor}.tcp-ping-stop-$1-$2"
}

tcp_ping() {
  local port="$1" stop got verdict deadline=$((SECONDS + 180))
  stop="$(stop_flag "$port" "${2:-0}")"
  got="$(mktemp)"
  while (( SECONDS < deadline )); do
    if [[ -e "$stop" ]]; then
      rm -f "$got" "$stop"
      echo "boot test: tcp-ping $port: the smoke is gone, not retrying" >&2
      return 1
    fi
    # The group keeps the connect error quiet without redirecting the
    # script's stderr for good, as `exec ... 2>/dev/null` would.
    if { exec 3<>"/dev/tcp/127.0.0.1/$port"; } 2>/dev/null; then
      printf ping >&3
      # Everything up to the smoke's close: "pong\n", then the lines.
      timeout 120 cat <&3 > "$got" 2>/dev/null || true
      exec 3>&-
      if [[ "$(head -c 5 "$got")" == pong ]]; then
        local diff
        diff="$(tail -c +6 "$got" | cmp - <(seq -f %06g 0 $((BULK_LINES - 1))) 2>&1)"
        if [[ -z "$diff" ]]; then
          verdict=good
        else
          # The byte count and where the stream first goes wrong.
          verdict="bad $(($(wc -c < "$got") - 5)): ${diff#*: }"
        fi
        echo "boot test: tcp-ping $port: pong, bulk $verdict" >&2
        if { exec 3<>"/dev/tcp/127.0.0.1/$port"; } 2>/dev/null; then
          printf '%s\n' "$verdict" >&3
          sleep 1
          exec 3>&-
        fi
        rm -f "$got" "$stop"
        [[ "$verdict" == good ]] || return 1
        tcp_mute "$port"
        return
      fi
    fi
    sleep 0.25
  done
  rm -f "$got" "$stop"
  echo "boot test: tcp-ping $port: no pong within the bound" >&2
  return 1
}

tcp_ping_stop() {
  : > "$(stop_flag "$1" "${2:-0}")"
}

# The mute connections (see the header), 0.3 s apart so the smoke's accept
# keeps up with netd's two parked handoffs, then "done".
# A connect or close that fails on the host is said (the smoke counts the
# connections it accepted: one lost tells little without the host's side).
tcp_mute() {
  python3 - "$1" <<'PY' >&2
import socket, sys, time
port = int(sys.argv[1])
failed = []
ports = []
for i in range(40):
    s = socket.socket()
    try:
        s.connect(("127.0.0.1", port))
        ports.append(str(s.getsockname()[1]))
        time.sleep(i * 0.0005)
        s.close()
    except OSError as e:
        failed.append(f"{i}: {e}")
    time.sleep(0.3)
s = socket.socket()
s.connect(("127.0.0.1", port))
done_port = s.getsockname()[1]
s.sendall(b"done")
time.sleep(1)
s.close()
# The ports are what the guest sees as the peers' (QEMU keeps them): a
# connection the smoke never got is named by the one missing from its list.
print(f"boot test: tcp-mute {port}: 40 mute connections ({len(failed)} failed on the host{': ' + ', '.join(failed) if failed else ''}), then done; ports {' '.join(ports)}, done {done_port}")
PY
}

sendkey() {
  python3 - "${MYOS_QEMU_MONITOR:?no QEMU monitor}" "$@" <<'PY'
import socket, sys, time
s = socket.socket(socket.AF_UNIX)
s.connect(sys.argv[1])
s.settimeout(5)
s.recv(4096)  # the banner and prompt
for key in sys.argv[2:]:
    key, _, hold = key.partition("@")
    s.sendall(f"sendkey {key} {hold}\n".encode() if hold else f"sendkey {key}\n".encode())
    time.sleep(0.5 + int(hold or 0) / 1000)
time.sleep(1)
s.close()
PY
  echo "boot test: sendkey $*" >&2
}

keyburst() {
  python3 - "${MYOS_QEMU_MONITOR:?no QEMU monitor}" "$1" "$2" <<'PY'
import socket, sys, time
s = socket.socket(socket.AF_UNIX)
s.connect(sys.argv[1])
s.settimeout(5)
s.recv(4096)  # the banner and prompt
for _ in range(int(sys.argv[2])):
    s.sendall(f"sendkey {sys.argv[3]}\n".encode())
    time.sleep(0.01)
time.sleep(1)
s.close()
PY
  echo "boot test: keyburst $1 $2" >&2
}

mouse() {
  python3 - "${MYOS_QEMU_MONITOR:?no QEMU monitor}" "${1:-usb}" <<'PY'
import re, socket, sys, time
s = socket.socket(socket.AF_UNIX)
s.connect(sys.argv[1])
s.settimeout(5)
s.recv(4096)  # the banner and prompt

def run(cmd):
    s.sendall(f"{cmd}\n".encode())
    time.sleep(0.5)
    try:
        return s.recv(65536).decode(errors="replace")
    except Exception:
        return ""

# "  Mouse #2: QEMU PS/2 Mouse", the active one starred.
mice = dict((name, n) for n, name in re.findall(r"Mouse #(\d+): (QEMU [^\r\n]+)", run("info mice")))
other = {"ps2": "QEMU PS/2 Mouse", "virtio": "QEMU Virtio Mouse"}.get(sys.argv[2])
if other:
    run(f"mouse_set {mice.get(other, '')}")
for cmd in ("mouse_move 10 -5", "mouse_button 1", "mouse_button 0"):
    run(cmd)
if other and "QEMU HID Mouse" in mice:
    run(f"mouse_set {mice['QEMU HID Mouse']}")
time.sleep(1)
s.close()
PY
  echo "boot test: mouse ${1:-usb}" >&2
}

usb_kbd() {
  local cmd
  case "$1" in
    plug) cmd="device_add usb-kbd,bus=${MYOS_USB_BUS:-xhci.0},port=${MYOS_USB_PORT:-}4,id=usbkbd" ;;
    unplug) cmd="device_del usbkbd" ;;
    *) echo "host.sh: usb-kbd plug|unplug: $1" >&2; return 2 ;;
  esac
  python3 - "${MYOS_QEMU_MONITOR:?no QEMU monitor}" "$cmd" <<'PY'
import socket, sys, time
s = socket.socket(socket.AF_UNIX)
s.connect(sys.argv[1])
s.settimeout(5)
s.recv(4096)  # the banner and prompt
s.sendall(f"{sys.argv[2]}\n".encode())
time.sleep(0.5)
try:
    for line in s.recv(65536).decode(errors="replace").splitlines():
        if line.strip().startswith("Error:"):
            print(f"boot test: monitor error: {line.strip()}", file=sys.stderr)
except Exception:
    pass
s.close()
PY
  echo "boot test: usb-kbd $1" >&2
}

case "${1:-}" in
  tcp-ping) tcp_ping "${2:?port}" "${3:-0}" ;;
  tcp-ping-stop) tcp_ping_stop "${2:?port}" "${3:-0}" ;;
  sendkey) shift; sendkey "${1:?keys}" "${@:2}" ;;
  keyburst) keyburst "${2:?count}" "${3:?key}" ;;
  mouse) mouse "${2:-usb}" ;;
  usb-kbd) usb_kbd "${2:?plug|unplug}" ;;
  *) echo "host.sh: unknown request: $*" >&2; exit 2 ;;
esac
