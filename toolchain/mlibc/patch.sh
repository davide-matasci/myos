#!/usr/bin/env bash
# Install the myos sysdeps into the fetched mlibc tree (as libgloss/myos is
# installed into newlib's), with the C both libcs share beside them, and
# make mlibc's build know the system: every edit of an upstream file is a
# python replacement here, applied once (patch.sh's way in toolchain/newlib).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
SRC="${MLIBC_SRC:-$ROOT/target/mlibc-src}"
PORT="$SRC/sysdeps/myos"

if [[ ! -f "$SRC/meson.build" ]]; then
  echo "mlibc source missing at $SRC (run toolchain/mlibc/fetch.sh)" >&2
  exit 1
fi

echo "==> install myos sysdeps -> $PORT"
rm -rf "$PORT"
mkdir -p "$PORT"
cp -a "$HERE/sysdeps/myos/." "$PORT/"
mkdir -p "$PORT/libmyos"
cp "$ROOT"/toolchain/libmyos/*.c "$ROOT"/toolchain/libmyos/*.h "$PORT/libmyos/"
# The crti/crtn of libgloss: the same .init/.fini bookends for both libcs.
for arch in x86_64 aarch64 riscv64; do
  cp "$ROOT/toolchain/newlib/libgloss/myos/crti-$arch.S" "$PORT/$arch/crti.S"
  cp "$ROOT/toolchain/newlib/libgloss/myos/crtn-$arch.S" "$PORT/$arch/crtn.S"
done

# The ABI bits: the Linux ones (abis/linux), except where myos has its own
# (the signal numbers and flags, the termios flags, checked in here).
for h in "$SRC"/abis/linux/*.h; do
  name="$(basename "$h")"
  [[ -e "$PORT/include/abi-bits/$name" ]] || ln -s "../../../../abis/linux/$name" "$PORT/include/abi-bits/$name"
done

# riscv64 userspace on myos is soft-float (rv64imac, docs/ports.md): setjmp
# saves the FP registers only where there are some, as fenv.S already does.
python3 - "$SRC/options/internal/riscv64/setjmp.S" <<'PY'
import re, sys
path = sys.argv[1]
s = open(path).read()
if "__riscv_flen" not in s:
    s = re.sub(r"(\tfsd fs0, 112\(a0\)\n(?:\tfsd fs\d+, \d+\(a0\)\n)+)", r"#ifdef __riscv_flen\n\1#endif\n", s, count=1)
    s = re.sub(r"(\tfld fs0,112\(a0\)\n(?:\tfld fs\d+,\d+\(a0\)\n)+)", r"#ifdef __riscv_flen\n\1#endif\n", s, count=1)
    assert s.count("__riscv_flen") == 2, "setjmp.S: the FP register blocks not found"
    open(path, "w").write(s)
    print("patched riscv64/setjmp.S: FP registers only with an FPU")
PY

# Without a feature macro mlibc's headers show POSIX 1990 and what
# _DEFAULT_SOURCE adds; newlib's (and glibc's) show POSIX 2008 with the XSI
# extensions (renameat, ...), which a program written for them expects.
python3 - "$SRC/mlibc-config.h.in" <<'PY'
import sys
path = sys.argv[1]
s = open(path).read()
if "The default (myos)" not in s:
    old = "#if !defined(_XOPEN_SOURCE) && _XOPEN_SOURCE_EXTENDED\n"
    new = """/* The default (myos): POSIX 2008 with the XSI extensions, as newlib's and
 * glibc's _DEFAULT_SOURCE. */
#if defined(_DEFAULT_SOURCE) && !defined(_POSIX_C_SOURCE) && !defined(_XOPEN_SOURCE)
#	define _XOPEN_SOURCE 700
#endif

""" + old
    assert s.count(old) == 1, "mlibc-config.h.in: the _XOPEN_SOURCE_EXTENDED block not found"
    open(path, "w").write(s.replace(old, new))
    print("patched mlibc-config.h.in: POSIX 2008 by default")
PY

# memfd_create is declared with the linux option only; myos has it
# (docs/mm.md, libmyos/shm.c) without that option.
python3 - "$SRC/options/posix/include/sys/mman.h" <<'PY'
import sys
path = sys.argv[1]
s = open(path).read()
if "Not Linux's alone" not in s:
    old = """#if __MLIBC_LINUX_OPTION && defined(_GNU_SOURCE)
void *mremap(void *__old_address, size_t __old_size, size_t __new_size, int __flags, ...);
int remap_file_pages(void *__addr, size_t __size, int __prot, size_t __pgoff, int __flags);
int memfd_create(const char *__name, unsigned int __flags);
#endif /* __MLIBC_LINUX_OPTION && defined(_GNU_SOURCE) */
"""
    new = """#if __MLIBC_LINUX_OPTION && defined(_GNU_SOURCE)
void *mremap(void *__old_address, size_t __old_size, size_t __new_size, int __flags, ...);
int remap_file_pages(void *__addr, size_t __size, int __prot, size_t __pgoff, int __flags);
#endif /* __MLIBC_LINUX_OPTION && defined(_GNU_SOURCE) */

/* Not Linux's alone (FreeBSD has it too): a sysdep without the linux
 * option provides it or not (myos does, docs/mm.md). */
#if defined(_GNU_SOURCE)
int memfd_create(const char *__name, unsigned int __flags);
#endif /* defined(_GNU_SOURCE) */
"""
    assert s.count(old) == 1, "sys/mman.h: the memfd_create block not found"
    open(path, "w").write(s.replace(old, new))
    print("patched sys/mman.h: memfd_create without the linux option")
PY

# The top-level meson.build picks the sysdeps by host_machine.system().
python3 - "$SRC/meson.build" <<'PY'
import sys
path = sys.argv[1]
s = open(path).read()
if "host_machine.system() == 'myos'" not in s:
    anchor = "# ANCHOR: demo-sysdeps\nelif host_machine.system() == 'demo'"
    assert s.count(anchor) == 1, "meson.build: demo sysdeps anchor not found"
    s = s.replace(anchor, """elif host_machine.system() == 'myos'
	rtld_include_dirs += include_directories('sysdeps/myos/include')
	libc_include_dirs += include_directories('sysdeps/myos/include')
	subdir('sysdeps/myos')
""" + anchor)
    open(path, "w").write(s)
    print("patched meson.build: the myos sysdeps")
PY
