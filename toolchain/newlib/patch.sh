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

patch_config_sub
patch_configure_host
patch_string_h_basename

echo "myos newlib patches applied"
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
