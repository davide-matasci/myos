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

# A locked mutex's destroy is EBUSY, an unlock of a mutex nobody holds EPERM
# and a thread's join of itself EDEADLK (POSIX lets them fail so; libgloss's
# do, and the pthread smoke checks it), not a panic or a wait.
python3 - "$SRC/options/internal/generic/threads.cpp" <<'PY'
import sys
path = sys.argv[1]
s = open(path).read()
old = "int thread_mutex_destroy(struct __mlibc_mutex *mutex) {\n\t__ensure(!mutex->__mlibc_state);\n\treturn 0;\n}\n"
new = "int thread_mutex_destroy(struct __mlibc_mutex *mutex) {\n\tif(mutex->__mlibc_state)\n\t\treturn EBUSY;\n\treturn 0;\n}\n"
if new not in s:
    assert s.count(old) == 1, "threads.cpp: thread_mutex_destroy not found"
    s = s.replace(old, new)
    open(path, "w").write(s)
    print("patched threads.cpp: EBUSY for a locked mutex's destroy")
# Likewise an unlock of a mutex nobody holds: EPERM, what POSIX says for
# an error-checking one.
old = "\t// Decrement the recursion level and unlock if we hit zero.\n\t__ensure(mutex->__mlibc_recursion);\n"
new = "\t// Decrement the recursion level and unlock if we hit zero.\n\tif(!mutex->__mlibc_recursion)\n\t\treturn EPERM;\n"
if new not in s:
    assert s.count(old) == 1, "threads.cpp: thread_mutex_unlock's check not found"
    s = s.replace(old, new)
    open(path, "w").write(s)
    print("patched threads.cpp: EPERM for an unlock of a mutex nobody holds")
# And a thread's join of itself: EDEADLK (POSIX's "may"), not a wait that
# never ends.
old = "\tmlibc::thread_testcancel();\n\n\twhile (!__atomic_load_n(&tcb->didExit, __ATOMIC_ACQUIRE)) {\n"
new = "\tif(tcb == mlibc::get_current_tcb())\n\t\treturn EDEADLK;\n\n" + old
if new not in s:
    assert s.count(old) == 1, "threads.cpp: thread_join's wait not found"
    open(path, "w").write(s.replace(old, new))
    print("patched threads.cpp: EDEADLK for a thread's join of itself")
PY

# hstrerror says what the h_errno codes mean (upstream's returns ""; the
# libc smoke, written to newlib's, expects the messages).
python3 - "$SRC/options/posix/generic/netdb.cpp" <<'PY'
import sys
path = sys.argv[1]
s = open(path).read()
old = 'const char *hstrerror(int) {\n\treturn "";\n}\n'
new = """const char *hstrerror(int err) {
\tswitch(err) {
\tcase HOST_NOT_FOUND: return "Unknown host";
\tcase TRY_AGAIN: return "Host name lookup failure";
\tcase NO_RECOVERY: return "Unknown server error";
\tcase NO_DATA: return "No address associated with name";
\tdefault: return "Unknown resolver error";
\t}
}
"""
if new not in s:
    assert s.count(old) == 1, "netdb.cpp: hstrerror not found"
    open(path, "w").write(s.replace(old, new))
    print("patched netdb.cpp: hstrerror's messages")
PY

# A read-write lock's writer is known (its tid in the lock word, where
# upstream writes 1): a thread's relock of its own write lock is EDEADLK
# and an unlock of a lock nobody holds EPERM (POSIX's "may"; libgloss's
# are, the pthread smoke checks them), not a wait that never ends.
python3 - "$SRC/options/posix/generic/pthread.cpp" <<'PY'
import sys
path = sys.argv[1]
s = open(path).read()
if "myos: a thread's relock" not in s:
    owner = "unsigned int desired = 1;\n"
    assert s.count(owner) == 2, "pthread.cpp: the rwlock's lock words not found"
    s = s.replace(owner, "unsigned int desired = mlibc::this_tid();\n")
    old = """int pthread_rwlock_wrlock(pthread_rwlock_t *rw) {
\tSCOPE_TRACE();

\t// Take the __mlibc_m mutex.
\t// Will be released in pthread_rwlock_unlock().
\tif (int e = rwlock_m_lock(rw, true); e)
"""
    new = """int pthread_rwlock_wrlock(pthread_rwlock_t *rw) {
\tSCOPE_TRACE();

\t// myos: a thread's relock of its own write lock is EDEADLK, not a wait.
\tif ((__mlibc_fair_rwlock::from(rw)->__mlibc_m.load(std::memory_order_relaxed)
\t\t\t& (__mlibc_fair_rwlock::mutex_owner_mask | __mlibc_fair_rwlock::mutex_excl_bit))
\t\t\t== (mlibc::this_tid() | __mlibc_fair_rwlock::mutex_excl_bit))
\t\treturn EDEADLK;
\t// Take the __mlibc_m mutex.
\t// Will be released in pthread_rwlock_unlock().
\tif (int e = rwlock_m_lock(rw, true); e)
"""
    assert s.count(old) == 1, "pthread.cpp: pthread_rwlock_wrlock not found"
    s = s.replace(old, new)
    old = """\tif(!rc_expected) {
\t\t// We are doing a write-unlock.
\t\trwlock_m_unlock(rw);
\t\treturn 0;
"""
    new = """\tif(!rc_expected) {
\t\t// We are doing a write-unlock.
\t\t// myos: of a lock nobody holds, EPERM.
\t\tif(!(l->__mlibc_m.load(std::memory_order_relaxed) & __mlibc_fair_rwlock::mutex_owner_mask))
\t\t\treturn EPERM;
\t\trwlock_m_unlock(rw);
\t\treturn 0;
"""
    assert s.count(old) == 1, "pthread.cpp: pthread_rwlock_unlock's write path not found"
    s = s.replace(old, new)
    open(path, "w").write(s)
    print("patched pthread.cpp: the rwlock's writer is known")
PY

# A cancellation inside a condition wait (mlibc cancels from its signal
# handler, in the wait) runs the cleanup handlers with the mutex locked
# again, as POSIX says: the wait is wrapped in an internal cleanup handler
# that takes the mutex. (Upstream's own TODO; the pthread smoke checks it.)
python3 - "$SRC/options/internal/generic/threads.cpp" <<'PY'
import sys
path = sys.argv[1]
s = open(path).read()
if "cond_timedwait_relocking" not in s:
    old = """int thread_cond_timedwait(struct __mlibc_cond *__restrict cond, __mlibc_mutex *__restrict mutex,
\t\tconst struct timespec *__restrict abstime, clockid_t clockid) {
\tmlibc::thread_testcancel();
"""
    new = """static int cond_timedwait_relocking(struct __mlibc_cond *__restrict cond, __mlibc_mutex *__restrict mutex,
\t\tconst struct timespec *__restrict abstime, clockid_t clockid);

// myos: a cancellation in the wait (the signal handler cancels there) runs
// the cleanup handlers with the mutex locked again, as POSIX says.
int thread_cond_timedwait(struct __mlibc_cond *__restrict cond, __mlibc_mutex *__restrict mutex,
\t\tconst struct timespec *__restrict abstime, clockid_t clockid) {
\tauto self = get_current_tcb();
\tauto relock = frg::construct<Tcb::CleanupHandler>(getAllocator());
\trelock->func = [] (void *m) { thread_mutex_lock(static_cast<__mlibc_mutex *>(m)); };
\trelock->arg = mutex;
\tself->cleanupHandlers.push_back(relock);
\tint e = cond_timedwait_relocking(cond, mutex, abstime, clockid);
\tself->cleanupHandlers.pop_back();
\tfrg::destruct(getAllocator(), relock);
\treturn e;
}

static int cond_timedwait_relocking(struct __mlibc_cond *__restrict cond, __mlibc_mutex *__restrict mutex,
\t\tconst struct timespec *__restrict abstime, clockid_t clockid) {
\tmlibc::thread_testcancel();
"""
    assert s.count(old) == 1, "threads.cpp: thread_cond_timedwait not found"
    open(path, "w").write(s.replace(old, new))
    print("patched threads.cpp: the condition wait relocks on a cancellation")
PY

# A failed write through stdio sets the stream's error indicator (what
# ferror reads) instead of a log line per failure: a program writing to a
# full filesystem in a loop (the pthread smoke's busy threads) otherwise
# floods the console and crawls.
python3 - "$SRC/options/ansi/generic/stdio.cpp" <<'PY'
import re, sys
path = sys.argv[1]
s = open(path).read()
if "myos: the stream's error indicator" not in s:
    pat = re.compile(r'(\t+)// TODO: Handle I/O errors\.\n\1mlibc::infoLogger\(\) << "mlibc: fwrite\(\) I/O errors are not handled"\n\t+<< frg::endlog;\n')
    s, n = pat.subn(r"\1// myos: the stream's error indicator, as ferror reads it.\n\1file->__status_bits |= __MLIBC_ERROR_BIT;\n", s)
    assert n == 2, "stdio.cpp: fwrite's error log lines not found (%d)" % n
    open(path, "w").write(s)
    print("patched stdio.cpp: a failed fwrite sets the error indicator")
PY

# The resolver's nameserver: /etc/resolv.conf as upstream, and without that
# file the first dns= of /net/ndb (netd's, from its lease), as libgloss's
# resolver (toolchain/newlib/libgloss/myos/netdb.c).
python3 - "$SRC/options/posix/generic/resolv_conf.cpp" <<'PY'
import sys
path = sys.argv[1]
s = open(path).read()
if "myos: without the file" not in s:
    old = """\tauto file = fopen("/etc/resolv.conf", "r");
\tif (!file)
\t\treturn frg::null_opt;
"""
    new = """\tauto file = fopen("/etc/resolv.conf", "r");
\tif (!file) {
\t\t// myos: without the file, the first dns= of /net/ndb (netd's, from its
\t\t// lease), as libgloss's resolver.
\t\tfile = fopen("/net/ndb", "r");
\t\tif (!file)
\t\t\treturn frg::null_opt;
\t\tchar ndb[1024];
\t\tsize_t n = fread(ndb, 1, sizeof ndb - 1, file);
\t\tfclose(file);
\t\tndb[n] = '\\0';
\t\tchar *dns = strstr(ndb, "dns=");
\t\tif (!dns)
\t\t\treturn frg::null_opt;
\t\tdns += 4;
\t\tstruct nameserver_data ret;
\t\tret.name = frg::string<MemoryAllocator>(dns, strcspn(dns, " \\t\\n"), getAllocator());
\t\treturn ret;
\t}
"""
    assert s.count(old) == 1, "resolv_conf.cpp: the open not found"
    s = s.replace(old, new)
    s = s.replace("#include <ctype.h>\n", "#include <ctype.h>\n#include <string.h>\n", 1)
    open(path, "w").write(s)
    print("patched resolv_conf.cpp: /net/ndb's dns= without /etc/resolv.conf")
PY

# The arc4random family is the shared C's (libmyos/random.c, as newlib has
# it): declared in <stdlib.h> with the BSD option's other calls, as the
# BSDs and glibc 2.36 do.
python3 - "$SRC/options/bsd/include/bits/bsd/bsd_stdlib.h" <<'PY'
import sys
path = sys.argv[1]
s = open(path).read()
if "arc4random" not in s:
    old = "#if defined(_DEFAULT_SOURCE)\nint getloadavg(double *__loadavg, int __count);\n#endif\n"
    new = """#if defined(_DEFAULT_SOURCE)
int getloadavg(double *__loadavg, int __count);
/* myos: the shared C's (toolchain/libmyos/random.c). */
__UINT32_TYPE__ arc4random(void);
void arc4random_buf(void *__buf, __SIZE_TYPE__ __n);
__UINT32_TYPE__ arc4random_uniform(__UINT32_TYPE__ __upper_bound);
#endif
"""
    assert s.count(old) == 1, "bsd_stdlib.h: the getloadavg block not found"
    open(path, "w").write(s.replace(old, new))
    print("patched bsd_stdlib.h: the arc4random family")
PY

# The top-level meson.build picks the sysdeps by host_machine.system().
# The kernel relocates a static PIE itself and applies RELATIVE relocations
# only (kernel/src/modules/elf.rs): no ifuncs (x86_64's strcmp), whose
# IRELATIVE it would refuse.
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
check = "\tifuncs_supported = cpp_compiler.compiles(ifunc_check, name: 'C++ compiler supports ifunc')\n"
off = "\tif host_machine.system() == 'myos'\n\t\tifuncs_supported = false\n\tendif\n"
if off not in s:
    assert s.count(check) == 1, "meson.build: the ifunc check not found"
    open(path, "w").write(s.replace(check, check + off))
    print("patched meson.build: no ifuncs")
PY

# inet_pton's IPv6 with an embedded IPv4 address (::ffff:10.0.2.2) checks
# each octet's leading zero against the hex group parsed before it, not the
# octet, so an octet "0" after the first is refused; each checks its own.
python3 - "$SRC/options/posix/generic/arpa-inet.cpp" <<'PY'
import sys
path = sys.argv[1]
s = open(path).read()
old = "if ((ipv4end - src) > 3 || (src[0] == '0' && value))"
if s.count(old) == 4:
    for k in range(4):
        s = s.replace(old, f"if ((ipv4end - src) > 3 || (src[0] == '0' && value{k}))", 1)
    open(path, "w").write(s)
    print("patched arpa-inet.cpp: inet_pton's IPv4 octets checked each")
else:
    assert "src[0] == '0' && value3" in s, "arpa-inet.cpp: inet_pton's octet checks not found"
PY
