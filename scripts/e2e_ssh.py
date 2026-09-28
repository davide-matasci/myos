#!/usr/bin/env python3
"""Focused riscv64 SSH-listener liveness e2e.

Boots target/riscv64.img, logs in, starts dropbear on :22, then hammers the
slirp hostfwd port with abandoned TCP connections (half-open/orphan churn) and
finally asserts a real pubkey SSH session still succeeds quickly. This targets
the CI #1164 failure mode: port :22 silently unanswerable for minutes.
"""
import os, socket, subprocess, sys, time

LOG = open("/tmp/e2e-ssh-serial.log", "ab")

def slog(b):
    LOG.write(b)
    LOG.flush()


ROOT = "/root/wt-stall"
IMG = f"{ROOT}/target/riscv64.img"
SOCK = "/tmp/e2e-ssh.sock"
QEMU = "qemu-system-riscv64"
CODE = "/usr/share/qemu-efi-riscv64/RISCV_VIRT_CODE.fd"
VARS = "/usr/share/qemu-efi-riscv64/RISCV_VIRT_VARS.fd"
for c in ("/usr/share/qemu-efi-riscv64/RISCV_VIRT_CODE.fd",
          "/usr/share/edk2/riscv64/RISCV_VIRT_CODE.fd"):
    if os.path.isfile(c):
        CODE = c
        VARS = c.replace("CODE", "VARS")
        break
KEY = f"{ROOT}/ports/dropbear/testkey"


def free2222():
    s = socket.socket()
    try:
        s.bind(("127.0.0.1", 2222))
        return True
    except OSError:
        return False
    finally:
        s.close()


def qemu_args():
    return [
        QEMU,
        "-global", "virtio-mmio.force-legacy=false",
        "-machine", "virt",
        "-cpu", "rv64",
        "-m", "4096",
        "-smp", "2",
        "-drive", f"if=pflash,format=raw,unit=0,file={CODE},readonly=on",
        "-drive", f"if=pflash,format=raw,unit=1,file={VARS},snapshot=on",
        "-drive", f"if=none,id=hd0,format=raw,file={IMG}",
        "-device", "virtio-blk-device,drive=hd0,bootindex=1",
        "-netdev", "user,id=net0,hostfwd=tcp::2222-:22",
        "-device", "virtio-net-pci,netdev=net0",
        "-serial", f"unix:{SOCK},server,nowait",
        "-nic", "none",
        "-no-reboot",
        "-accel", "tcg,thread=single",
    ]


def drive(ser, deadline):
    """Login and start dropbear over the serial socket."""
    buf = b""
    while b"login:" not in buf and time.time() < deadline:
        try:
            b = ser.recv(4096)
        except socket.timeout:
            continue
        if not b:
            break
        buf += b
        slog(b)
    if b"login:" not in buf:
        return False, "no login prompt"
    ser.sendall(b"root\n")
    ser.settimeout(20)
    buf = b""
    while b"Password:" not in buf and time.time() < deadline:
        try:
            b = ser.recv(4096)
        except socket.timeout:
            return False, "no password prompt"
        if not b:
            break
        buf += b
        slog(b)
    if b"Password:" not in buf:
        return False, "no password prompt"
    ser.sendall(b"\n")
    buf = b""
    while b"$" not in buf[-4096:] and time.time() < deadline:
        try:
            b = ser.recv(4096)
        except socket.timeout:
            continue
        if not b:
            break
        buf += b
        slog(b)
    if b"$" not in buf[-4096:]:
        return False, "no shell prompt"
    ser.sendall(b"/bin/custom/dropbear -F -E -p 22 -r /etc/dropbear/ed25519_hostkey"
                b" > /tmp/dropbear.out 2>&1 & echo DROPBEAR-UP\n")
    # Give dropbear time to arm listen.
    for _ in range(12):
        try:
            b = ser.recv(4096)
        except socket.timeout:
            b = b""
        if b:
            slog(b)
        time.sleep(1)
    return True, "ok"


def hammer(n=25):
    """Abandoned connects: SYN (maybe handshake) then immediate close."""
    closed = 0
    for _ in range(n):
        try:
            s = socket.create_connection(("127.0.0.1", 2222), timeout=5)
            s.close()
            closed += 1
        except OSError:
            pass
        time.sleep(0.05)
    return closed


def real_ssh():
    r = subprocess.run(
        ["timeout", "25", "ssh", "-4", "-i", KEY, "-p", "2222",
         "-o", "StrictHostKeyChecking=no", "-o", "UserKnownHostsFile=/dev/null",
         "-o", "GlobalKnownHostsFile=/dev/null", "-o", "BatchMode=yes",
         "-o", "IdentitiesOnly=yes", "-o", "PreferredAuthentications=publickey",
         "-o", "KexAlgorithms=curve25519-sha256", "-o", "ConnectTimeout=8",
         "-o", "HostKeyAlgorithms=ssh-ed25519",
         "-o", "ConnectionAttempts=1", "root@127.0.0.1", "echo E2E-OK; /bin/coreutils/true"],
        capture_output=True, text=True,
    )
    return r.returncode == 0 and "E2E-OK" in r.stdout, r.returncode, r.stderr[-300:]


def main():
    if not os.path.isfile(IMG):
        print("FAIL: image missing", IMG)
        return 1
    if not free2222():
        os.system("pkill -9 -f '[q]emu-system-riscv'")
        time.sleep(1)
        if not free2222():
            print("FAIL: host :2222 busy")
            return 1
    try:
        os.unlink(SOCK)
    except FileNotFoundError:
        pass
    q = subprocess.Popen(qemu_args(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        deadline = time.time() + 240
        ser = None
        while time.time() < deadline:
            try:
                ser = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                ser.connect(SOCK)
                break
            except OSError:
                time.sleep(0.5)
        if ser is None:
            print("FAIL: serial socket never came up")
            return 1
        ser.settimeout(5)
        ok, why = drive(ser, deadline)
        if not ok:
            print("FAIL: login/dropbear:", why)
            return 1
        print("dropbear up; hammering :2222 with abandoned connects")
        closed = hammer(25)
        print(f"abandoned connects: {closed}/25")
        time.sleep(1)
        ok2, rc, err = real_ssh()
        print(f"real ssh rc={rc} ok={ok2} stderr={err!r}")
        if ok2:
            print("E2E PASS")
            return 0
        print("E2E FAIL: ssh after hammer did not succeed")
        return 1
    finally:
        q.terminate()
        try:
            q.wait(timeout=10)
        except Exception:
            q.kill()


if __name__ == "__main__":
    sys.exit(main())
