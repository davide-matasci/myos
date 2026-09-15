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

patch_valist
