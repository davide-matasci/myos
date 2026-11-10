#!/usr/bin/env python3
"""A sampling profile of a booted kernel, through the QEMU monitor
(docs/testing.md, "Measuring").

    scripts/prof.py sample LOG OUT          # beside the boot, until QEMU exits
    scripts/prof.py report OUT KERNEL_ELF [PHASE [LINES]]

`sample` connects to the monitor socket the launcher gives every boot
(/tmp/myos-monitor-<pid>.sock) and asks for every CPU's registers a few
hundred times a second: a line of program counters per sample in OUT. LOG is
the boot's console output, being written (`cargo run -- ... > LOG`): each new
line of it with `BENCH` in it goes into OUT as a mark, so the samples
between two marks are those of one phase of user/c/bench.c, which prints a
line as each ends.

`report` names the functions the samples fall in, with addr2line on the
kernel the boot ran (x86_64: target/x86_64-unknown-none/debug/deps/artifact/
kernel-*/bin/kernel-*; elsewhere target/<triple>/debug/kernel), inlined
callers after `<`: for every phase, or for the last one whose mark has PHASE
in it. A CPU that was halted counts as `<halted>` (x86_64, where the monitor
says so), an address outside the kernel's image as `<user or module>`.

Under TCG a program counter is that of the translated block a CPU is in,
and the cost of an instruction is the emulator's, not a CPU's: the profile
shows where the time goes, and which lock a CPU spins on, not what an
MMIO write or an exit from a guest costs on hardware.
"""
import collections
import glob
import re
import socket
import subprocess
import sys
import time


def sample(log, out):
    sock = None
    while sock is None:
        for path in glob.glob("/tmp/myos-monitor-*.sock"):
            try:
                s = socket.socket(socket.AF_UNIX)
                s.connect(path)
                sock = s
                break
            except OSError:
                pass
        time.sleep(0.5)
    sock.settimeout(2)

    def registers():
        sock.sendall(b"info registers -a\n")
        buf = b""
        while b"info registers" not in buf or not buf.rstrip().endswith(b"(qemu)"):
            try:
                data = sock.recv(65536)
            except socket.timeout:
                break
            if not data:
                raise EOFError
            buf += data
        return buf.decode(errors="replace")

    try:
        sock.recv(65536)
    except socket.timeout:
        pass
    seen = 0
    with open(out, "w") as o:
        try:
            while True:
                text = registers()
                pcs = (
                    re.findall(r"RIP=([0-9a-f]+)", text)
                    or re.findall(r"PC=([0-9a-f]+)", text)
                    or re.findall(r"\bpc\s+([0-9a-f]+)", text)
                )
                halted = re.findall(r"HLT=(\d)", text)
                try:
                    with open(log, errors="replace") as f:
                        lines = f.read().split("\n")
                except OSError:
                    lines = []
                for line in lines[seen:]:
                    if "BENCH" in line:
                        o.write("MARK " + line.strip() + "\n")
                seen = max(seen, len(lines) - 1)
                o.write("S " + " ".join(pcs) + " H " + "".join(halted) + "\n")
                o.flush()
                time.sleep(0.002)
        except (EOFError, OSError):
            pass


def report(prof, elf, want, lines):
    phases, cur = [], []
    with open(prof) as f:
        for line in f:
            if line.startswith("MARK"):
                phases.append((line[5:].strip(), cur))
                cur = []
            elif line.startswith("S "):
                pcs, _, halted = line[2:].partition(" H ")
                cur.append((pcs.split(), halted.strip()))
    phases = [(mark, samples) for mark, samples in phases if len(samples) > 20]
    if want:
        phases = [p for p in phases if want in p[0]][-1:]
    addrs = sorted({pc for _, samples in phases for pcs, _ in samples for pc in pcs})
    chains, at = {}, None
    if addrs:
        out = subprocess.run(
            ["addr2line", "-i", "-f", "-C", "-a", "-e", elf] + ["0x" + a for a in addrs],
            capture_output=True, text=True, check=False,
        ).stdout
        for line in out.split("\n"):
            if line.startswith("0x"):
                at = int(line, 16)
                chains[at] = []
            elif line and at is not None and not line.startswith(("/", "?")):
                chains[at].append(re.sub(r"<.*?>", "", line.split("::{")[0])[-40:])
    for mark, samples in phases:
        count, total = collections.Counter(), 0
        for pcs, halted in samples:
            for i, pc in enumerate(pcs):
                chain = chains.get(int(pc, 16))
                if i < len(halted) and halted[i] == "1":
                    count["<halted>"] += 1
                elif chain:
                    count[" < ".join(chain[:6])] += 1
                else:
                    count["<user or module>"] += 1
                total += 1
        print(f"== {mark} ({len(samples)} samples)")
        for what, n in count.most_common(lines):
            print(f"  {100.0 * n / total:5.1f}%  {what}")


def main(argv):
    if len(argv) == 4 and argv[1] == "sample":
        sample(argv[2], argv[3])
    elif len(argv) in (4, 5, 6) and argv[1] == "report":
        report(argv[2], argv[3], argv[4] if len(argv) > 4 else "", int(argv[5]) if len(argv) > 5 else 20)
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main(sys.argv)
