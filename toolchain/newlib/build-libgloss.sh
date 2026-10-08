#!/usr/bin/env bash
# Build myos libgloss.a + crt0.o for one architecture.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
arch="${1:?arch}"
prefix="${2:?prefix}"
triple="${arch}-unknown-myos"
NEWLIB_SRC="$ROOT/target/newlib-src"
PORT="$NEWLIB_SRC/libgloss/myos"
CC="${triple}-cc"
libdir="$prefix/${triple}/lib"
inc="$prefix/${triple}/include"
out="$ROOT/target/libgloss-myos-${arch}"

rm -rf "$out"
mkdir -p "$out/obj" "$libdir" "$inc" "$inc/sys"
cp "$ROOT/toolchain/newlib/libgloss/myos/crt0-${arch}.S" "$PORT/crt0.S"
cp "$ROOT/toolchain/newlib/libgloss/myos/crti-${arch}.S" "$PORT/crti.S"
cp "$ROOT/toolchain/newlib/libgloss/myos/crtn-${arch}.S" "$PORT/crtn.S"
# termios.c needs <termios.h> in the sysroot before compile.
cp "$ROOT/toolchain/newlib/libgloss/myos/termios.h" "$inc/termios.h"
# Sync the patched libc headers (search.h qelem/lsearch, endian.h shim,
# regex.h sys/types, machine/setjmp.h sigsetjmp) into the sysroot so
# host-prebuilds and guest tcc see the same declarations as libgloss.a.
for hdr in search.h endian.h regex.h; do
  if [[ -f "$NEWLIB_SRC/newlib/libc/include/$hdr" ]]; then
    cp "$NEWLIB_SRC/newlib/libc/include/$hdr" "$inc/$hdr"
  fi
done
# newlib defines _POSIX_THREADS (which guards <pthread.h>), the UNIX98
# mutex types and _POSIX_TIMERS (which guards clock_gettime, nanosleep and
# the CLOCK_* ids) only for RTEMS and Cygwin. libgloss implements the API
# (pthread.c), so the installed features.h declares it for myos; every
# program sees the same pthread_mutexattr_t. Only the sysroot copy: newlib's
# own build keeps them off (its stdio would call pthread_setcancelstate
# around every lock).
cp "$NEWLIB_SRC/newlib/libc/include/sys/features.h" "$inc/sys/features.h"
python3 - "$inc/sys/features.h" <<'PY'
import sys
path = sys.argv[1]
s = open(path).read()
tail = "#ifdef __cplusplus\n}\n#endif\n#endif /* _SYS_FEATURES_H */"
assert s.count(tail) == 1, "features.h tail not found"
s = s.replace(tail, """/* myos: libgloss pthread.c. */
#ifndef _POSIX_THREADS
#define _POSIX_THREADS 1
#endif
#ifndef _UNIX98_THREAD_MUTEX_ATTRIBUTES
#define _UNIX98_THREAD_MUTEX_ATTRIBUTES 1
#endif
/* myos: libgloss time.c (clock_gettime, CLOCK_REALTIME and
 * CLOCK_MONOTONIC) and sleep.c (nanosleep); <time.h> declares them only
 * with these. */
#ifndef _POSIX_TIMERS
#define _POSIX_TIMERS 1
#endif
#ifndef _POSIX_MONOTONIC_CLOCK
#define _POSIX_MONOTONIC_CLOCK 200112L
#endif

""" + tail)
open(path, "w").write(s)
PY
# The thread types libgloss's pthread.c needs wider than newlib's 32-bit
# words: pthread_t is the thread's control block, a mutex keeps its holder
# and recursion count beside its lock word. The rest (a condition variable,
# once, keys, attributes) is newlib's.
cp "$NEWLIB_SRC/newlib/libc/include/sys/_pthreadtypes.h" "$inc/sys/_pthreadtypes.h"
python3 - "$inc/sys/_pthreadtypes.h" <<'PY'
import sys
path = sys.argv[1]
s = open(path).read()
edits = [
    ("typedef __uint32_t pthread_t;            /* identify a thread */",
     "typedef struct __pthread *pthread_t;     /* myos: the thread's control block */"),
    ("typedef __uint32_t pthread_mutex_t;      /* identify a mutex */",
     """typedef struct {                         /* myos: libgloss pthread.c */
  __uint32_t __state;   /* 0 free, 1 held, 2 held with waiters (wait_addr) */
  __uint32_t __type;    /* PTHREAD_MUTEX_* */
  __uint32_t __owner;   /* the holder's tid */
  __uint32_t __count;   /* a recursive mutex's locks beyond the first */
} pthread_mutex_t;"""),
    ("#define _PTHREAD_MUTEX_INITIALIZER ((pthread_mutex_t) 0xFFFFFFFF)",
     "#define _PTHREAD_MUTEX_INITIALIZER { 0, 3, 0, 0 } /* PTHREAD_MUTEX_DEFAULT */"),
]
for old, new in edits:
    assert s.count(old) == 1, f"_pthreadtypes.h: {old!r} not found"
    s = s.replace(old, new)
open(path, "w").write(s)
PY
# Programs reach errno and stdio's state through __getreent(), as newlib
# itself was built (build.sh): each thread has its own.
python3 - "$inc/sys/config.h" <<'PY'
import sys
path = sys.argv[1]
s = open(path).read()
mark = "/* myos: per-thread reent */"
if mark not in s:
    tail = "#endif /* __SYS_CONFIG_H__ */"
    assert s.count(tail) == 1, "config.h tail not found"
    s = s.replace(tail, mark + "\n#ifndef __DYNAMIC_REENT__\n#define __DYNAMIC_REENT__\n#endif\n\n" + tail)
    open(path, "w").write(s)
PY
if [[ -f "$NEWLIB_SRC/newlib/libc/include/machine/setjmp.h" ]]; then
  mkdir -p "$inc/machine"
  cp "$NEWLIB_SRC/newlib/libc/include/machine/setjmp.h" "$inc/machine/setjmp.h"
fi
# Sync ALL current libgloss sources into the fetched newlib tree before
# compiling: the build compiles from $PORT (the fetched copy), and without
# this a source edit under toolchain/newlib/libgloss/myos/ never reaches
# libgloss.a (pwdgrp.c sat stale here for a week, shipping a getpwent()
# that returned NULL — the os-test setpwent regression).
# Cache-bust: netd ctl taken + accept-arm-only (PR #164 riscv64 SSH).
# Cache-bust: pollselect infinite-wait gettimeofday yield (dropbear/netd).
# Cache-bust: sleep.c (SYS_NANOSLEEP) + pollselect sleeping between scans.
for src_f in "$ROOT"/toolchain/newlib/libgloss/myos/*.c "$ROOT"/toolchain/newlib/libgloss/myos/*.h; do
  cp "$src_f" "$PORT/"
done
# -I"$PORT" shadows the sysroot: sync sys/*.h too (dirent.c needs DT_FIFO).
mkdir -p "$PORT/sys"
cp "$ROOT"/toolchain/newlib/libgloss/myos/sys/*.h "$PORT/sys/"

for f in myos_raw syscalls stubs posix_stubs posix_extra misc_stubs more_stubs signal ioctl environ getline dirent at basename dirname time pwdgrp mmap mount termios ttyctl socket inet netdb pollselect pty search sleep uio pthread syslog reboot; do
  "$CC" -ffreestanding -fPIC -O2 -I"$PORT" -isystem "$inc" \
    -c "$PORT/${f}.c" -o "$out/obj/${f}.o"
done

# POSIX regex (regcomp/regexec/regerror/regfree + engine) from newlib's own
# libc/posix — not wired into this newlib target's libc build, but os-test
# regex/*.c need the full API. Compile the upstream sources verbatim.
if [[ -d "$NEWLIB_SRC/newlib/libc/posix" ]]; then
  # collate.c + collcmp.c back the locale hooks regex uses
  # (__collate_load_error, __collate_range_cmp); same -I as the regex sources.
  for rf in regcomp regexec regerror regfree collate collcmp; do
    "$CC" -ffreestanding -fPIC -O2 -isystem "$inc" \
      -I"$NEWLIB_SRC/newlib/libc/posix" \
      -c "$NEWLIB_SRC/newlib/libc/posix/${rf}.c" -o "$out/obj/${rf}.o"
  done
fi
"$CC" -c "$PORT/crt0.S" -o "$out/obj/crt0.o"
"$CC" -c "$PORT/crti.S" -o "$out/obj/crti.o"
"$CC" -c "$PORT/crtn.S" -o "$out/obj/crtn.o"

# crt*.o are standalone CRT objects, not members of libgloss.a
# Use llvm-ar when present: Apple's BSD ar writes ELF archive symbol tables
# rust-lld cannot index (undefined getpid/write/sbrk at link time on macOS).
AR_BIN="$(command -v llvm-ar 2>/dev/null || echo ar)"
"$AR_BIN" rcs "$out/libgloss.a" "$out/obj"/*.o
"$AR_BIN" d "$out/libgloss.a" crti.o crtn.o 2>/dev/null || true
cp "$out/libgloss.a" "$libdir/libgloss.a"
# The pthread functions are in libgloss (pthread.c); an empty libpthread.a
# keeps `-lpthread` in ported Makefiles linking.
rm -f "$libdir/libpthread.a"
"$AR_BIN" rcs "$libdir/libpthread.a"
cp "$out/obj/crt0.o" "$libdir/crt0.o"
cp "$out/obj/crti.o" "$libdir/crti.o"
cp "$out/obj/crtn.o" "$libdir/crtn.o"
mkdir -p "$libdir/specs" "$inc/sys"
cp "$PORT/myos.specs" "$libdir/specs/myos.specs"
cp "$ROOT/toolchain/newlib/libgloss/myos/sys/dirent.h" "$inc/sys/dirent.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/sys/syslimits.h" "$inc/sys/syslimits.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/sys/sysmacros.h" "$inc/sys/sysmacros.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/sys/myos_extra.h" "$inc/sys/myos_extra.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/sys/ioctl.h" "$inc/sys/ioctl.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/sys/socket.h" "$inc/sys/socket.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/sys/un.h" "$inc/sys/un.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/sys/utsname.h" "$inc/sys/utsname.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/sys/mman.h" "$inc/sys/mman.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/sys/uio.h" "$inc/sys/uio.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/sys/random.h" "$inc/sys/random.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/sys/utime.h" "$inc/sys/utime.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/sys/reboot.h" "$inc/sys/reboot.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/sys/syslog.h" "$inc/sys/syslog.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/syslog.h" "$inc/syslog.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/utmp.h" "$inc/utmp.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/termios.h" "$inc/termios.h"
mkdir -p "$inc/arpa" "$inc/netinet"
cp "$ROOT/toolchain/newlib/libgloss/myos/arpa/inet.h" "$inc/arpa/inet.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/netinet/in.h" "$inc/netinet/in.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/netdb.h" "$inc/netdb.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/poll.h" "$inc/poll.h"
cp "$ROOT/toolchain/newlib/libgloss/myos/pty.h" "$inc/pty.h"
echo "libgloss-myos -> $libdir/libgloss.a"
