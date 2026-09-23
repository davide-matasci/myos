#!/usr/bin/env bash
# Install the in-tree myos libgloss port into a fetched newlib source tree.
# NOTE: no `sed -i` here — GNU and BSD sed disagree about -i syntax and
# multiline a/i commands, which broke macOS builds ("extra characters at the
# end of d command"). All in-place edits go through patch_edit (python3).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
NEWLIB_SRC="${NEWLIB_SRC:-$ROOT/target/newlib-src}"
PORT="$ROOT/toolchain/newlib/libgloss/myos"

if [[ ! -d "$NEWLIB_SRC/newlib" ]]; then
  echo "newlib source missing at $NEWLIB_SRC (run toolchain/newlib/fetch.sh)" >&2
  exit 1
fi

echo "==> install myos libgloss port -> $NEWLIB_SRC/libgloss/myos"
rm -rf "$NEWLIB_SRC/libgloss/myos"
mkdir -p "$NEWLIB_SRC/libgloss/myos"
cp -a "$PORT"/. "$NEWLIB_SRC/libgloss/myos/"

patch_string_h_basename() {
  # tcc (non-GCC) does not define __GNUC__, so newlib's cdefs.h never defines
  # __ASMNAME. string.h's basename alias then expands to
  # `__asm__(__ASMNAME("__gnu_basename"))` with __ASMNAME unresolved, and tcc's
  # asm-label parser fails with "string constant expected" (seen compiling
  # os-test basic/pwd/setpwent). Guard the alias on __ASMNAME being defined.
  local f="$NEWLIB_SRC/newlib/libc/include/string.h"
  if grep -q 'basename (const char \*) __asm__(__ASMNAME' "$f" \
     && ! grep -q 'basename-asmname-guard' "$f"; then
    python3 - "$f" <<'EOF'
import sys
f = sys.argv[1]
s = open(f).read()
old = '''#if __GNU_VISIBLE && !defined(basename)
# define basename basename
char\t*__nonnull ((1)) basename (const char *) __asm__(__ASMNAME("__gnu_basename"));
#endif'''
new = '''#if __GNU_VISIBLE && !defined(basename)
# define basename basename
/* basename-asmname-guard: only GCC defines __ASMNAME (via cdefs.h); plain
   compilers get the plain declaration. */
#ifdef __ASMNAME
char\t*__nonnull ((1)) basename (const char *) __asm__(__ASMNAME("__gnu_basename"));
#else
char\t*__nonnull ((1)) basename (const char *);
#endif
#endif'''
assert old in s, "string.h basename block not found"
open(f, 'w').write(s.replace(old, new, 1))
EOF
    echo "patched string.h: guard basename asm alias on __ASMNAME"
  fi
}

patch_edit_all() {
  # Replace EVERY occurrence (>=1 required). Args: file old new
  python3 - "$@" <<'PYEDIT'
import sys
path, old, new = sys.argv[1], sys.argv[2], sys.argv[3]
s = open(path).read()
assert old in s, f"{path}: pattern not found: {old[:60]!r}"
open(path, "w").write(s.replace(old, new))
PYEDIT
}

patch_edit() {
  # Portable in-place edit. Args: file old new count
  # python3 is a build prerequisite on every host (Linux CI + macOS).
  python3 - "$@" <<'PYEDIT'
import sys
path, old, new, count = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
s = open(path).read()
got = s.count(old)
assert got >= count, f"{path}: expected >= {count} occurrence(s) of {old[:60]!r}, found {got}"
if got > count:
    # Replace only the first `count` occurrences (e.g. configure.host has
    # several '  *)' catch-alls; only the first insertion point is wanted).
    parts = s.split(old)
    s = old.join(parts[:count]) + new + old.join(parts[count:])
    open(path, "w").write(s)
else:
    open(path, "w").write(s.replace(old, new))
PYEDIT
}

patch_config_sub() {
  local f="$NEWLIB_SRC/config.sub"
  if grep -q 'midnightbsd\* | amdhsa\* | unleashed\* | emscripten\* | wasi\* \\' "$f" \
     && ! grep -q 'myos\*' "$f"; then
    patch_edit "$f" \
      'unleashed* | emscripten* | wasi* \' \
      'unleashed* | emscripten* | wasi* \
	     | myos* \' 1
    echo "patched config.sub for myos"
  fi
}

patch_configure_host() {
  local f="$NEWLIB_SRC/newlib/configure.host"
  if grep -q '\*-\*-myos\*)' "$f"; then
    if grep -q 'HAVE_FCNTL' "$f" && ! grep -q 'HAVE_RENAME' "$f"; then
      patch_edit "$f" '-DHAVE_FCNTL' '-DHAVE_FCNTL -DHAVE_RENAME' 1
      echo "patched newlib/configure.host myos: added HAVE_RENAME"
      return
    fi
    if ! grep -q 'HAVE_FCNTL' "$f"; then
      # Insert flags after syscall_dir=syscalls inside the myos arm.
      patch_edit "$f" 'syscall_dir=syscalls' \
        'syscall_dir=syscalls\
	newlib_cflags="${newlib_cflags} -DHAVE_FCNTL -DHAVE_RENAME"' 1
      echo "patched newlib/configure.host myos for HAVE_FCNTL HAVE_RENAME"
    fi
    return
  fi
  # configure.host has several case blocks; the old GNU sed inserted the
  # myos arm before EVERY '  *)' catch-all and CI depended on the arm in the
  # final (newlib_cflags / syscall_dir) block. Keep inserting before all of
  # them so every block handles myos like it did on Linux CI.
  patch_edit_all "$f" \
    '  *)' \
    '  *-*-myos*)\
	syscall_dir=syscalls\
	newlib_cflags="${newlib_cflags} -DHAVE_FCNTL -DHAVE_RENAME"\
	;;\
  *)'
  echo "patched newlib/configure.host for myos"
}

# <endian.h> shim: newlib ships <sys/endian.h> (htobe*/le*/bswap* macros +
# __bswap builtins via <machine/endian.h>) but no top-level <endian.h>, so
# os-test's endian/*.c fall back to the HOST /usr/include/endian.h and die on
# glibc's bits/wordsize.h. glibc/musl both expose <endian.h> as the primary
# header — provide the same (a real wrapper, no re-implementation).
install_endian_h() {
  local f="$NEWLIB_SRC/newlib/libc/include/endian.h"
  if [[ ! -f "$f" ]]; then
    cat > "$f" <<'EOH'
/* myos: <endian.h> is the primary public header (glibc/musl convention).
 * newlib's implementations live in <sys/endian.h>. */
#ifndef _MYOS_ENDIAN_H_
#define _MYOS_ENDIAN_H_
#include <sys/endian.h>
#endif
EOH
    echo "installed endian.h shim (-> sys/endian.h)"
  fi
}

# regex.h uses off_t (regoff_t) without pulling in <sys/types.h>; standalone
# inclusion fails with "unknown type name off_t".
patch_regex_h() {
  local f="$NEWLIB_SRC/newlib/libc/include/regex.h"
  if grep -q 'typedef off_t regoff_t' "$f" && ! grep -q 'regex-sys-types' "$f"; then
    patch_edit "$f" 'typedef off_t regoff_t;' \
'#include <sys/types.h> /* regex-sys-types: regoff_t needs off_t */
typedef off_t regoff_t;' 1
    echo "patched regex.h: include <sys/types.h>"
  fi
}

# POSIX sigsetjmp/siglongjmp: newlib gates the sigjmp_buf typedef + macros on
# (__CYGWIN__ || __rtems__) && __POSIX_VISIBLE. Enable them for myos targets
# too (kernel implements SYS_SIGPROCMASK; macros are the POSIX implementation).
patch_setjmp_h() {
  local f="$NEWLIB_SRC/newlib/libc/include/machine/setjmp.h"
  if grep -q '#if (defined(__CYGWIN__) || defined(__rtems__)) && __POSIX_VISIBLE' "$f" \
     && ! grep -q 'setjmp-myos-posix' "$f"; then
    patch_edit "$f" \
      '#if (defined(__CYGWIN__) || defined(__rtems__)) && __POSIX_VISIBLE' \
      '#if __POSIX_VISIBLE /* setjmp-myos-posix: enable on every target; newlib gates this on CYGWIN/RTEMS but POSIX requires it everywhere */' 1
    echo "patched machine/setjmp.h: enable sigsetjmp/siglongjmp for myos"
  fi
  # features.h must be in scope for __POSIX_VISIBLE when <setjmp.h> is the
  # first include; sys/cdefs.h pulls it in but only via _ansi.h ordering.
  # Ensure visibility by including <sys/features.h> before the guarded block.
  if ! grep -q 'setjmp-features' "$f"; then
    patch_edit "$f" '/* POSIX sigsetjmp/siglongjmp macros */' \
'#include <sys/features.h> /* setjmp-features: __POSIX_VISIBLE must be defined */

/* POSIX sigsetjmp/siglongjmp macros */' 1
    echo "patched machine/setjmp.h: include <sys/features.h>"
  fi
}

# search.h (newlib) lacks lsearch/lfind/insque/remque + struct qelem that
# POSIX puts in <search.h>; os-test search/*.c need them. Declare them and
# implement in libgloss myos search.c.
patch_search_h() {
  local f="$NEWLIB_SRC/newlib/libc/include/search.h"
  if ! grep -q 'search-lsearch-qelem' "$f"; then
    patch_edit "$f" '__END_DECLS' \
'/* search-lsearch-qelem: POSIX lsearch/lfind/insque/remque (libgloss myos). */
struct qelem {
	struct qelem *q_forw;
	struct qelem *q_back;
	char *q_data;
};

void	insque(void *, void *);
void	remque(void *);
void	*lfind(const void *, const void *, size_t *, size_t,
	    int (*)(const void *, const void *));
void	*lsearch(const void *, void *, size_t *, size_t,
	    int (*)(const void *, const void *));

__END_DECLS' 1
    echo "patched search.h: qelem + lsearch/insque declarations"
  fi
}

patch_config_sub
patch_configure_host
patch_string_h_basename
install_endian_h
patch_search_h
patch_regex_h
patch_setjmp_h

echo "myos newlib patches applied"


# x86_64 newlib longjmp(env, 0) must make setjmp return 1 (POSIX). Upstream
# setjmp.S just moves rsi→rax, so longjmp(buf, 0) looks like a first setjmp
# return — os-test basic/setjmp/longjmp and the savemask=0 half of siglongjmp
# fail. aarch64 (cinc) and riscv (seqz+add) already handle this.
patch_x86_longjmp_val0() {
  local f="$NEWLIB_SRC/newlib/libc/machine/x86_64/setjmp.S"
  if [[ -f "$f" ]] && ! grep -q 'longjmp-myos-val0' "$f"; then
    python3 - "$f" <<'PYLJ'
import sys
f = sys.argv[1]
s = open(f).read()
old = """SYM (longjmp):
  movq    rsi, rax        /* Return value */

  movq     8 (rdi), rbp"""
new = """SYM (longjmp):
  movq    rsi, rax        /* Return value */
  /* longjmp-myos-val0: POSIX — longjmp(env, 0) makes setjmp return 1 */
  testq   rax, rax
  jnz     .Lmyos_lj_ok
  movq    $1, rax
.Lmyos_lj_ok:

  movq     8 (rdi), rbp"""
assert old in s, f"{f}: longjmp prolog not found"
open(f, "w").write(s.replace(old, new, 1))
PYLJ
    echo "patched x86_64 setjmp.S: longjmp(env,0) -> setjmp returns 1"
  fi
}

# tmpfile (newlib) unlinks immediately; the myos kernel drops tmpfs content
# with the last name, so later writes on the fd fail (stdio/fflush: EIO).
# POSIX only requires the file be discarded at program termination: defer the
# unlink to atexit (myos-tmpfile-defer).
patch_tmpfile () {
  local f="$NEWLIB_SRC/newlib/libc/stdio/tmpfile.c"
  if ! grep -q 'myos-tmpfile-defer' "$f"; then
    python3 - "$f" <<'PYTMPFILE'
import sys
f = sys.argv[1]
s = open(f).read()
anchor = "FILE *\n_tmpfile_r (struct _reent *ptr)"
prelude = """/* myos-tmpfile-defer: keep the name until program exit; the kernel
 * drops tmpfs content when the last link disappears, which breaks later
 * writes on the tmpfile fd. POSIX requires discard at termination only. */
#include <stdlib.h>
static char myos_tmpfile_keep[L_tmpnam];
static void myos_tmpfile_unlink (void) { remove (myos_tmpfile_keep); }

FILE *
_tmpfile_r (struct _reent *ptr)"""
assert anchor in s, f"{f}: _tmpfile_r anchor not found"
s = s.replace(anchor, prelude, 1)
old = "  (void) _remove_r (ptr, f);"
new = """  {
    size_t k = 0;
    while (f[k] != '\0' && k < sizeof (myos_tmpfile_keep) - 1) {
      myos_tmpfile_keep[k] = f[k];
      k++;
    }
    myos_tmpfile_keep[k] = '\0';
    atexit (myos_tmpfile_unlink);
  }"""
assert old in s, f"{f}: _remove_r line not found"
s = s.replace(old, new, 1)
open(f, "w").write(s)
PYTMPFILE
    echo "patched tmpfile.c: defer unlink to exit (myos-tmpfile-defer)"
  fi
}

patch_valist() {
  # tcc does not define __GNUC__, so newlib's stdio.h / wchar.h fall back to
  # '#define __VALIST char*'. But our tcc provides a GCC-compatible va_list
  # (struct __va_list_tag[1] from tccdefs.h), so the fallback type is wrong:
  # every vfprintf/vfwprintf-style call warns "assignment from incompatible
  # pointer type". Always use __gnuc_va_list, which both tcc's stdarg.h and
  # GNU/clang stdarg.h define consistently.
  for f in "$NEWLIB_SRC/newlib/libc/include/stdio.h" \
           "$NEWLIB_SRC/newlib/libc/include/wchar.h"; do
    grep -q '#define __VALIST char\*' "$f" || continue
    python3 - "$f" <<'PYVALIST'
import sys
f = sys.argv[1]
s = open(f).read()
old = """#ifndef __VALIST
#ifdef __GNUC__
#define __VALIST __gnuc_va_list
#else
#define __VALIST char*
#endif
#endif"""
new = """#ifndef __VALIST
#define __VALIST __gnuc_va_list
#endif"""
assert old in s, f"{f}: __VALIST block not found"
open(f, "w").write(s.replace(old, new, 1))
PYVALIST
    echo "patched $(basename "$f"): __VALIST always __gnuc_va_list (tcc has no __GNUC__)"
  done
}

patch_limits_myos() {
  # POSIX limit macros for myos. The sortix/os-test limits suite compiles
  # each test as `#ifdef X ... X >= required-minimum ... #else missing_optional`
  # and expects the constants to exist with honest values. Without this block
  # ~100 tests either fail to compile (macros referenced inside the #ifdef
  # branch) or fail at runtime with "missing_optional". Values mirror kernel
  # reality where measurable (OPEN_MAX=MAX_FDS, CHILD_MAX=MAX_TASKS).
  local f="$NEWLIB_SRC/newlib/libc/include/limits.h"
  if ! grep -q 'myos-posix-limits-begin' "$f"; then
    python3 - "$f" <<'PYLIMITS'
import sys
f = sys.argv[1]
s = open(f).read()
block = r'''
/* myos-posix-limits-begin: POSIX/XSI limit constants for myos. The os-test
   limits suite verifies each is declared with at least its POSIX minimum. */
#ifndef _POSIX_AIO_LISTIO_MAX
#define _POSIX_AIO_LISTIO_MAX	2
#endif
#ifndef _POSIX_AIO_MAX
#define _POSIX_AIO_MAX		1
#endif
#ifndef _POSIX_ARG_MAX
#define _POSIX_ARG_MAX		4096
#endif
#ifndef _POSIX_CHILD_MAX
#define _POSIX_CHILD_MAX	25
#endif
#ifndef _POSIX_CLOCKRES_MIN
#define _POSIX_CLOCKRES_MIN	20000000
#endif
#ifndef _POSIX_DELAYTIMER_MAX
#define _POSIX_DELAYTIMER_MAX	32
#endif
#ifndef _POSIX_HOST_NAME_MAX
#define _POSIX_HOST_NAME_MAX	255
#endif
#ifndef _POSIX_LINK_MAX
#define _POSIX_LINK_MAX		8
#endif
#ifndef _POSIX_LOGIN_NAME_MAX
#define _POSIX_LOGIN_NAME_MAX	9
#endif
#ifndef _POSIX_MAX_CANON
#define _POSIX_MAX_CANON	255
#endif
#ifndef _POSIX_MAX_INPUT
#define _POSIX_MAX_INPUT	255
#endif
#ifndef _POSIX_MQ_OPEN_MAX
#define _POSIX_MQ_OPEN_MAX	8
#endif
#ifndef _POSIX_MQ_PRIO_MAX
#define _POSIX_MQ_PRIO_MAX	32
#endif
#ifndef _POSIX_NAME_MAX
#define _POSIX_NAME_MAX		14
#endif
#ifndef _POSIX_NGROUPS_MAX
#define _POSIX_NGROUPS_MAX	8
#endif
#ifndef _POSIX_OPEN_MAX
#define _POSIX_OPEN_MAX		20
#endif
#ifndef _POSIX_PATH_MAX
#define _POSIX_PATH_MAX		256
#endif
#ifndef _POSIX_PIPE_BUF
#define _POSIX_PIPE_BUF		512
#endif
#ifndef _POSIX_RE_DUP_MAX
#define _POSIX_RE_DUP_MAX	255
#endif
#ifndef _POSIX_RTSIG_MAX
#define _POSIX_RTSIG_MAX	8
#endif
#ifndef _POSIX_SEM_NSEMS_MAX
#define _POSIX_SEM_NSEMS_MAX	256
#endif
#ifndef _POSIX_SEM_VALUE_MAX
#define _POSIX_SEM_VALUE_MAX	32767
#endif
#ifndef _POSIX_SIGQUEUE_MAX
#define _POSIX_SIGQUEUE_MAX	32
#endif
#ifndef _POSIX_SSIZE_MAX
#define _POSIX_SSIZE_MAX	32767
#endif
#ifndef _POSIX_SS_REPL_MAX
#define _POSIX_SS_REPL_MAX	4
#endif
#ifndef _POSIX_STREAM_MAX
#define _POSIX_STREAM_MAX	8
#endif
#ifndef _POSIX_SYMLINK_MAX
#define _POSIX_SYMLINK_MAX	255
#endif
#ifndef _POSIX_SYMLOOP_MAX
#define _POSIX_SYMLOOP_MAX	8
#endif
#ifndef _POSIX_TEXTDOMAIN_MAX
#define _POSIX_TEXTDOMAIN_MAX	1024
#endif
#ifndef _POSIX_THREAD_DESTRUCTOR_ITERATIONS
#define _POSIX_THREAD_DESTRUCTOR_ITERATIONS	4
#endif
#ifndef _POSIX_THREAD_KEYS_MAX
#define _POSIX_THREAD_KEYS_MAX	128
#endif
#ifndef _POSIX_THREAD_THREADS_MAX
#define _POSIX_THREAD_THREADS_MAX	64
#endif
#ifndef _POSIX_TIMER_MAX
#define _POSIX_TIMER_MAX	32
#endif
#ifndef _POSIX_TTY_NAME_MAX
#define _POSIX_TTY_NAME_MAX	9
#endif
#ifndef _POSIX_TZNAME_MAX
#define _POSIX_TZNAME_MAX	6
#endif
#ifndef _POSIX2_BC_BASE_MAX
#define _POSIX2_BC_BASE_MAX	99
#endif
#ifndef _POSIX2_BC_DIM_MAX
#define _POSIX2_BC_DIM_MAX	255
#endif
#ifndef _POSIX2_BC_SCALE_MAX
#define _POSIX2_BC_SCALE_MAX	99
#endif
#ifndef _POSIX2_BC_STRING_MAX
#define _POSIX2_BC_STRING_MAX	255
#endif
#ifndef _POSIX2_CHARCLASS_NAME_MAX
#define _POSIX2_CHARCLASS_NAME_MAX	14
#endif
#ifndef _POSIX2_COLL_WEIGHTS_MAX
#define _POSIX2_COLL_WEIGHTS_MAX	255
#endif
#ifndef _POSIX2_EXPR_NEST_MAX
#define _POSIX2_EXPR_NEST_MAX	32
#endif
#ifndef _POSIX2_LINE_MAX
#define _POSIX2_LINE_MAX	2048
#endif
#ifndef _XOPEN_IOV_MAX
#define _XOPEN_IOV_MAX		16
#endif
#ifndef _XOPEN_NAME_MAX
#define _XOPEN_NAME_MAX		255
#endif
#ifndef _XOPEN_PATH_MAX
#define _XOPEN_PATH_MAX		1024
#endif
#ifndef _XOPEN_TEXTDOMAIN_MAX
#define _XOPEN_TEXTDOMAIN_MAX	1024
#endif

#ifndef AIO_LISTIO_MAX
#define AIO_LISTIO_MAX		2
#endif
#ifndef AIO_MAX
#define AIO_MAX			1
#endif
#ifndef AIO_PRIO_DELTA_MAX
#define AIO_PRIO_DELTA_MAX	0
#endif
#ifndef ATEXIT_MAX
#define ATEXIT_MAX		32
#endif
#ifndef BC_BASE_MAX
#define BC_BASE_MAX		99
#endif
#ifndef BC_DIM_MAX
#define BC_DIM_MAX		255
#endif
#ifndef BC_SCALE_MAX
#define BC_SCALE_MAX		99
#endif
#ifndef BC_STRING_MAX
#define BC_STRING_MAX		255
#endif
#ifndef CHARCLASS_NAME_MAX
#define CHARCLASS_NAME_MAX	14
#endif
/* CHILD_MAX: kernel MAX_TASKS. */
#ifndef CHILD_MAX
#define CHILD_MAX		32
#endif
#ifndef COLL_WEIGHTS_MAX
#define COLL_WEIGHTS_MAX	255
#endif
#ifndef DELAYTIMER_MAX
#define DELAYTIMER_MAX		32
#endif
#ifndef EXPR_NEST_MAX
#define EXPR_NEST_MAX		32
#endif
#ifndef FILESIZEBITS
#define FILESIZEBITS		64
#endif
#ifndef GETENTROPY_MAX
#define GETENTROPY_MAX		256
#endif
#ifndef HOST_NAME_MAX
#define HOST_NAME_MAX		255
#endif
#ifndef IOV_MAX
#define IOV_MAX			1024
#endif
#ifndef LINE_MAX
#define LINE_MAX		2048
#endif
#ifndef LINK_MAX
#define LINK_MAX		128
#endif
#ifndef LOGIN_NAME_MAX
#define LOGIN_NAME_MAX		256
#endif
#ifndef LONG_BIT
#define LONG_BIT		64
#endif
#ifndef MAX_CANON
#define MAX_CANON		255
#endif
#ifndef MAX_INPUT
#define MAX_INPUT		255
#endif
#ifndef MQ_OPEN_MAX
#define MQ_OPEN_MAX		8
#endif
#ifndef MQ_PRIO_MAX
#define MQ_PRIO_MAX		32
#endif
#ifndef NAME_MAX
#define NAME_MAX		255
#endif
#ifndef NL_LANGMAX
#define NL_LANGMAX		32
#endif
#ifndef NL_MSGMAX
#define NL_MSGMAX		32767
#endif
#ifndef NL_SETMAX
#define NL_SETMAX		255
#endif
#ifndef NL_TEXTMAX
#define NL_TEXTMAX		2048
#endif
#ifndef NZERO
#define NZERO			20
#endif
/* OPEN_MAX: kernel MAX_FDS. */
#ifndef OPEN_MAX
#define OPEN_MAX		32
#endif
#ifndef PAGESIZE
#define PAGESIZE		4096
#endif
#ifndef PAGE_SIZE
#define PAGE_SIZE		4096
#endif
#ifndef PTHREAD_DESTRUCTOR_ITERATIONS
#define PTHREAD_DESTRUCTOR_ITERATIONS	4
#endif
#ifndef PTHREAD_KEYS_MAX
#define PTHREAD_KEYS_MAX	128
#endif
#ifndef PTHREAD_STACK_MIN
#define PTHREAD_STACK_MIN	16384
#endif
#ifndef PTHREAD_THREADS_MAX
#define PTHREAD_THREADS_MAX	64
#endif
#ifndef RE_DUP_MAX
#define RE_DUP_MAX		255
#endif
#ifndef RTSIG_MAX
#define RTSIG_MAX		32
#endif
#ifndef SEM_NSEMS_MAX
#define SEM_NSEMS_MAX		256
#endif
#ifndef SEM_VALUE_MAX
#define SEM_VALUE_MAX		32767
#endif
#ifndef SIGQUEUE_MAX
#define SIGQUEUE_MAX		32
#endif
#ifndef SSIZE_MAX
#define SSIZE_MAX		((long long)(~((unsigned long long)0 >> 1)))
#endif
#ifndef SS_REPL_MAX
#define SS_REPL_MAX		4
#endif
#ifndef STREAM_MAX
#define STREAM_MAX		16
#endif
#ifndef SYMLINK_MAX
#define SYMLINK_MAX		255
#endif
#ifndef SYMLOOP_MAX
#define SYMLOOP_MAX		8
#endif
#ifndef TEXTDOMAIN_MAX
#define TEXTDOMAIN_MAX		1024
#endif
#ifndef TIMER_MAX
#define TIMER_MAX		32
#endif
#ifndef TTY_NAME_MAX
#define TTY_NAME_MAX		32
#endif
#ifndef TZNAME_MAX
#define TZNAME_MAX		32
#endif
#ifndef WORD_BIT
#define WORD_BIT		32
#endif
/* myos-posix-limits-end */'''
s = s.rstrip() + "\n" + block + "\n"
open(f, "w").write(s)
PYLIMITS
    echo "patched limits.h: myos POSIX limits block"
  fi
}

patch_valist
patch_tmpfile
patch_x86_longjmp_val0
patch_limits_myos
