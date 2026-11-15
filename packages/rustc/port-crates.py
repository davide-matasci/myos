#!/usr/bin/env python3
"""Teach the crates rustc and cargo are built from about myos.

usage: port-crates.py VENDOR_DIR LIBC_PATCH

The compiler's source carries its dependencies (`vendor/`). The ones that
list the systems they know get `target_os = "myos"` where a unix on a C
library with Linux's interfaces belongs: most often beside Hurd, the
other system that is neither Linux nor a BSD and has them. Each rule is
(crate directory glob, file glob, text, replacement); a rule that finds
nothing is an error, so that a bump of the compiler shows what moved.
A changed crate's checksum list is emptied, as cargo checks a vendored
crate's files against it.

The libc crate itself gets LIBC_PATCH, the one std's own copy has
(toolchain/std/patches/libc-myos.patch): the compiler and cargo are built
on the version it is for (rust-myos.myos.patch moves their lock files to
it).
"""

from __future__ import annotations

import glob
import json
import os
import subprocess
import sys

HURD = 'target_os = "hurd"'
# A predicate, so it stands wherever the one it replaces does: in a list,
# alone, or under `not`.
HURD_MYOS = 'any(target_os = "hurd", target_os = "myos")'

# (crates, files, old, new)
RULES: list[tuple[str, str, str, str]] = [
    # getrandom: the C library's getrandom(), and its errno.
    ("getrandom-0.[234].*", "src/**/*.rs", HURD, HURD_MYOS),
    ("getrandom-0.[34].*", "Cargo.toml", HURD, HURD_MYOS),
    # rustix: a unix it does not list gets its plain POSIX code, which is
    # right but for two places: ioctl's request is an unsigned long, and
    # there are no fallocate flags.
    ("rustix-1.*", "src/ioctl/mod.rs", HURD, HURD_MYOS),
    ("rustix-1.*", "src/backend/libc/fs/types.rs", HURD, HURD_MYOS),
    # ... and no statfs (myos has statvfs): the list of the systems
    # without it, which gates the call and its type.
    (
        "rustix-1.*",
        "src/**/*.rs",
        '    target_os = "horizon",\n    target_os = "netbsd",\n    target_os = "nto",\n    target_os = "redox",\n    target_os = "vita",\n    target_os = "wasi",\n)))]\n',
        '    target_os = "horizon",\n    target_os = "myos",\n    target_os = "netbsd",\n    target_os = "nto",\n    target_os = "redox",\n    target_os = "vita",\n    target_os = "wasi",\n)))]\n',
    ),
    (
        "rustix-1.*",
        "src/backend/libc/fs/dir.rs",
        '        target_os = "horizon",\n        target_os = "netbsd",\n        target_os = "nto",\n        target_os = "redox",\n        target_os = "vita",\n        target_os = "wasi",\n    )))]\n',
        '        target_os = "horizon",\n        target_os = "myos",\n        target_os = "netbsd",\n        target_os = "nto",\n        target_os = "redox",\n        target_os = "vita",\n        target_os = "wasi",\n    )))]\n',
    ),
    # The terminal: no CMSPAR (as Hurd), and no cfsetspeed in mlibc: the
    # two calls it stands for.
    (
        "rustix-1.*",
        "src/termios/types.rs",
        '            target_os = "hurd",\n            target_os = "nto",\n            target_os = "redox",\n        )))]\n        const CMSPAR = c::CMSPAR;',
        '            target_os = "hurd",\n            target_os = "myos",\n            target_os = "nto",\n            target_os = "redox",\n        )))]\n        const CMSPAR = c::CMSPAR;',
    ),
    (
        "rustix-1.*",
        "src/backend/libc/termios/syscalls.rs",
        """    #[cfg(not(linux_kernel))]
    unsafe {
        ret(c::cfsetspeed(
            as_mut_ptr(termios).cast(),
            encoded_speed.into(),
        ))
    }
""",
        """    #[cfg(target_os = "myos")]
    unsafe {
        ret(c::cfsetispeed(as_mut_ptr(termios).cast(), encoded_speed.into()))?;
        ret(c::cfsetospeed(as_mut_ptr(termios).cast(), encoded_speed.into()))
    }

    #[cfg(not(any(linux_kernel, target_os = "myos")))]
    unsafe {
        ret(c::cfsetspeed(
            as_mut_ptr(termios).cast(),
            encoded_speed.into(),
        ))
    }
""",
    ),
    # socket2: the socket types and options Hurd has (msg_iovlen an int).
    ("socket2-0.6.*", "src/sys/unix.rs", HURD, HURD_MYOS),
    ("socket2-0.6.*", "src/socket.rs", HURD, HURD_MYOS),
    # errno: the C library's __errno_location.
    ("errno-0.3.*", "src/unix.rs", HURD, HURD_MYOS),
    # libloading: dlopen's flags are mlibc's, the values Hurd has too.
    ("libloading-0.*", "src/os/unix/consts.rs", HURD, HURD_MYOS),
]


LIBC = "libc-0.2.189"


def main() -> None:
    vendor, libc_patch = sys.argv[1], sys.argv[2]
    touched: set[str] = set()
    libc = os.path.join(vendor, LIBC)
    if not os.path.isdir(libc):
        sys.exit(f"port-crates: no {LIBC} in {vendor}: rebase libc-myos.patch")
    if not os.path.isdir(os.path.join(libc, "src/unix/linux_like/myos")):
        subprocess.run(
            ["patch", "-p1", "-s", "--no-backup-if-mismatch", "-F0", "-i", os.path.abspath(libc_patch)],
            cwd=libc,
            check=True,
        )
    touched.add(libc)
    for crates, files, old, new in RULES:
        dirs = sorted(glob.glob(os.path.join(vendor, crates)))
        if not dirs:
            sys.exit(f"port-crates: no crate matches {crates}")
        for d in dirs:
            hits = 0
            for path in sorted(glob.glob(os.path.join(d, files), recursive=True)):
                text = open(path).read()
                if new in text:
                    hits += 1  # already ported
                elif old in text:
                    open(path, "w").write(text.replace(old, new))
                    hits += 1
            if not hits:
                sys.exit(f"port-crates: {os.path.basename(d)}: nothing to change in {files}")
            touched.add(d)
    for d in sorted(touched):
        checksum = os.path.join(d, ".cargo-checksum.json")
        sums = json.load(open(checksum))
        sums["files"] = {}
        json.dump(sums, open(checksum, "w"))
        print(f"ported {os.path.basename(d)}")


if __name__ == "__main__":
    main()
