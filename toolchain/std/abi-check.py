#!/usr/bin/env python3
"""Compare the libc crate's myos module with mlibc's headers.

The mlibc flavour of the Rust std (`<arch>-unknown-myos-mlibc`) calls the C
library through the libc crate (patches/libc-myos.patch), whose constants
and struct layouts are written by hand: a number that differs from the
header's is a wrong signal, flag or field at run time, with nothing to say
so at build time. This reads every constant, type and struct of the
crate's myos module (and of the modules it shares with Linux), asks clang
for the C side's value, size or field offset against mlibc's headers, and
compiles the crate with an assertion for each. A name the headers do not
have is skipped: the crate declares more of Linux than myos has.

usage: abi-check.py ARCH   (through abi-check.sh, which sets the build up)
"""

from __future__ import annotations

import glob
import os
import re
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ARCH = sys.argv[1]
TRIPLE = f"{ARCH}-unknown-myos-mlibc"
SYSROOT = Path(os.environ.get("MYOS_SYSROOT", ROOT / "target/myos-sysroot"))
LIBC = SYSROOT / "lib/rustlib/src/rust/library/vendor/libc-0.2.189"
CFLAGS = os.environ["MYOS_ABI_CFLAGS"].split()
OUT = ROOT / f"target/std-abi-check-{ARCH}"

HEADERS = """errno.h fcntl.h signal.h unistd.h sys/mman.h sys/stat.h sys/socket.h
netinet/in.h netinet/tcp.h netinet/ip.h arpa/inet.h sys/un.h termios.h
sys/ioctl.h poll.h sys/wait.h sys/time.h time.h sys/resource.h dirent.h
pthread.h sched.h limits.h netdb.h sys/select.h sys/epoll.h sys/eventfd.h
sys/uio.h dlfcn.h locale.h stdio.h stdlib.h string.h sys/utsname.h
sys/statvfs.h sys/file.h spawn.h pwd.h grp.h langinfo.h syslog.h sys/ipc.h
sys/shm.h sys/timerfd.h sys/signalfd.h net/if.h ifaddrs.h semaphore.h
utime.h fnmatch.h glob.h regex.h wchar.h sys/random.h link.h elf.h
sys/param.h stddef.h""".split()
PRELUDE = "#define _GNU_SOURCE\n#include <sys/types.h>\n" + "".join(
    f"#if __has_include(<{h}>)\n#include <{h}>\n#endif\n" for h in HEADERS
)
# Initializers, not numbers.
SKIP = {"IN6ADDR_ANY_INIT", "IN6ADDR_LOOPBACK_INIT"}
# Types whose name in C is also a function's (`sizeof` takes the function).
SKIP_TYPES = {"stat64"}


def sources() -> list[Path]:
    src = LIBC / "src"
    files = [
        src / "unix/linux_like/myos/mod.rs",
        src / f"unix/linux_like/myos/{ARCH}/mod.rs",
        *map(Path, sorted(glob.glob(str(src / "new/mlibc/*.rs")))),
        *map(Path, sorted(glob.glob(str(src / "new/common/posix/*.rs")))),
        *map(Path, sorted(glob.glob(str(src / "new/common/linux_like/*.rs")))),
        src / "unix/linux_like/mod.rs",
        src / "unix/mod.rs",
    ]
    return [f for f in files if f.exists()]


def clang_values(body: str) -> dict[str, int]:
    """The values of the `const long long v_NAME` definitions of `body`."""
    with tempfile.TemporaryDirectory() as d:
        c, s = Path(d, "t.c"), Path(d, "t.s")
        c.write_text(PRELUDE + body)
        r = subprocess.run(["clang", *CFLAGS, "-w", "-S", "-o", s, c], capture_output=True, text=True)
        if r.returncode:
            return {}
        return {
            m.group(1): int(m.group(2), 0)
            for m in re.finditer(r"^v_(\w+):\n\s+\.(?:quad|xword|dword)\s+(-?\w+)", s.read_text(), re.M)
        }


def c_value(expr: str) -> int | None:
    return clang_values(f"const long long v_x = (long long)({expr});\n").get("x")


def main() -> None:
    consts: set[str] = set()
    types: set[str] = set()
    structs: dict[str, list[str]] = {}
    for f in sources():
        text = f.read_text()
        for m in re.finditer(r"pub const ([A-Z_][A-Za-z0-9_]*)\s*:\s*([^=;]+)=", text):
            ty = m.group(2)
            if not any(x in ty for x in ("sighandler", "*", "[", "pthread", "Pthread")):
                consts.add(m.group(1))
        types.update(re.findall(r"^pub type (\w+) = ", text, re.M))
        for m in re.finditer(r"pub (?:struct|union) (\w+)\s*\{(.*?)\n    \}", text, re.S):
            fields = re.findall(r"^\s*pub (\w+)\s*:", m.group(2), re.M)
            structs.setdefault(m.group(1), fields)
    consts -= SKIP
    types -= SKIP_TYPES

    asserts = ["#![allow(unused, deprecated, overflowing_literals)]", "use core::mem::{offset_of, size_of};"]
    values = clang_values(
        "".join(f"#ifdef {n}\nconst long long v_{n} = (long long)({n});\n#endif\n" for n in sorted(consts))
    )
    if not values:
        sys.exit("abi-check: the constants did not compile against mlibc's headers")
    for n, v in sorted(values.items()):
        # Compared as 32 bits: a flag of the top bit is negative in a c_int.
        asserts.append(f'const _: () = assert!(libc::{n} as i128 as i32 == {v}i128 as i32, "{n}: C has {v}");')

    def probe_type(n: str) -> list[str]:
        v = c_value(f"sizeof({n})")
        return [] if v is None else [f'const _: () = assert!(size_of::<libc::{n}>() == {v}, "sizeof({n}): C has {v}");']

    def probe_struct(item: tuple[str, list[str]]) -> list[str]:
        n, fields = item
        for cname in (f"struct {n}", n, f"union {n}"):
            v = c_value(f"sizeof({cname})")
            if v is not None:
                break
        else:
            return []
        out = [f'const _: () = assert!(size_of::<libc::{n}>() == {v}, "sizeof({n}): C has {v}");']
        for fl in fields:
            o = c_value(f"offsetof({cname}, {fl})")
            if o is not None:
                out.append(f'const _: () = assert!(offset_of!(libc::{n}, {fl}) == {o}, "offsetof({n}, {fl}): C has {o}");')
        return out

    with ThreadPoolExecutor(os.cpu_count()) as pool:
        for lines in pool.map(probe_type, sorted(types)):
            asserts += lines
        for lines in pool.map(probe_struct, sorted(structs.items())):
            asserts += lines

    (OUT / "src").mkdir(parents=True, exist_ok=True)
    (OUT / "src/lib.rs").write_text("\n".join(asserts) + "\n")
    (OUT / "Cargo.toml").write_text(
        '[package]\nname = "std-abi-check"\nversion = "0.0.0"\nedition = "2021"\npublish = false\n\n'
        f'[dependencies]\nlibc = {{ path = "{LIBC}", default-features = false }}\n\n[workspace]\n'
    )
    r = subprocess.run(
        ["cargo", f"+{os.environ['MYOS_NIGHTLY']}", "build", "-q", "-Z", "unstable-options",
         "-Z", "json-target-spec", "--target", str(ROOT / f"targets/{TRIPLE}.json"),
         "--target-dir", str(OUT / "target"), "--manifest-path", str(OUT / "Cargo.toml")],
        capture_output=True, text=True,
    )
    wrong = sorted(set(re.findall(r"evaluation panicked: (.*)", r.stderr)))
    # A name of the shared modules that this target does not have, or a
    # private field: not a difference.
    other = [
        l for l in r.stderr.splitlines()
        if l.startswith("error") and not re.match(r"error(\[E0080\]|\[E0425\]|\[E0616\]|\[E0412\]|: could not compile)", l)
    ]
    for w in wrong:
        print(f"abi-check ({ARCH}): {w}")
    for l in other:
        print(f"abi-check ({ARCH}): {l}")
    if wrong or other:
        sys.exit(1)
    print(f"abi-check ({ARCH}): {len(asserts) - 2} constants, sizes and offsets agree with mlibc's headers")


if __name__ == "__main__":
    main()
