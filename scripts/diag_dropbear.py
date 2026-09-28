#!/usr/bin/env python3
"""Diagnostic: boot riscv64, start dropbear in foreground on serial, observe fault."""
import os, socket, subprocess, sys, time

ROOT = "/root/wt-stall"
IMG = f"{ROOT}/target/riscv64.img"
SOCK = "/tmp/diag.sock"
CODE = "/usr/share/qemu-efi-riscv64/RISCV_VIRT_CODE.fd"
VARS = "/usr/share/qemu-efi-riscv64/RISCV_VIRT_VARS.fd"

args = [
    "qemu-system-riscv64",
    "-global", "virtio-mmio.force-legacy=false",
    "-machine", "virt", "-cpu", "rv64", "-m", "4096", "-smp", "2",
    "-drive", f"if=pflash,format=raw,unit=0,file={CODE},readonly=on",
    "-drive", f"if=pflash,format=raw,unit=1,file={VARS},snapshot=on",
    "-drive", f"if=none,id=hd0,format=raw,file={IMG}",
    "-device", "virtio-blk-device,drive=hd0,bootindex=1",
    "-netdev", "user,id=net0",
    "-device", "virtio-net-pci,netdev=net0",
    "-chardev", f"socket,id=s0,path={SOCK},server=on,wait=off,logfile=/tmp/diag-serial.log",
    "-serial", "chardev:s0",
    "-nic", "none", "-no-reboot", "-accel", "tcg,thread=single",
]
try:
    os.unlink(SOCK)
except FileNotFoundError:
    pass
q = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
buf = b""
try:
    ser = None
    dl = time.time() + 240
    while time.time() < dl:
        try:
            ser = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            ser.connect(SOCK)
            break
        except OSError:
            time.sleep(0.5)
    ser.settimeout(3)
    def readfor(sec):
        global buf
        end = time.time() + sec
        while time.time() < end:
            try:
                b = ser.recv(4096)
                if b:
                    buf += b
            except socket.timeout:
                pass
    readfor(120)
    if b"login:" in buf:
        ser.sendall(b"root\n"); readfor(20)
        if b"Password:" in buf:
            ser.sendall(b"\n")
        readfor(30)
    print("--- serial before dropbear ---")
    print(buf[-1500:].decode("utf-8", "replace"))
    ser.sendall(b"/bin/custom/dropbear -F -E -p 22 -r /etc/dropbear/ed25519_hostkey > /tmp/dropbear.out 2>&1 & echo started\n")
    readfor(25)
    ser.sendall(b"cat /tmp/dropbear.out\n")
    readfor(10)
    print("--- serial after dropbear foreground ---")
    print(buf[-2500:].decode("utf-8", "replace"))
finally:
    q.terminate()
