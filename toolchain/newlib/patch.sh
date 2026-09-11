#!/usr/bin/env bash
# Install the in-tree myos libgloss port into a fetched newlib source tree.
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

patch_config_sub() {
  local f="$NEWLIB_SRC/config.sub"
  if grep -q 'midnightbsd\* | amdhsa\* | unleashed\* | emscripten\* | wasi\* \\' "$f" \
     && ! grep -q 'myos\*' "$f"; then
    sed -i 's/midnightbsd\* | amdhsa\* | unleashed\* | emscripten\* | wasi\* \\/&\n\t     | myos* \\/' "$f"
    echo "patched config.sub for myos"
  fi
}

patch_configure_host() {
  local f="$NEWLIB_SRC/newlib/configure.host"
  if grep -q '\*-\*-myos\*)' "$f"; then
    if grep -q 'HAVE_FCNTL' "$f" && ! grep -q 'HAVE_RENAME' "$f"; then
      sed -i 's/-DHAVE_FCNTL/-DHAVE_FCNTL -DHAVE_RENAME/g' "$f"
      echo "patched newlib/configure.host myos: added HAVE_RENAME"
      return
    fi
    if ! grep -q 'HAVE_FCNTL' "$f"; then
      # Insert flags after syscall_dir=syscalls inside the myos arm.
      sed -i '/\*-\\*-myos\*)/,/;;/{
        /syscall_dir=syscalls/a\
\tnewlib_cflags="${newlib_cflags} -DHAVE_FCNTL -DHAVE_RENAME"
      }' "$f"
      echo "patched newlib/configure.host myos for HAVE_FCNTL HAVE_RENAME"
    fi
    return
  fi
  sed -i '/^  \*)$/i\
  *-*-myos*)\
\tsyscall_dir=syscalls\
\tnewlib_cflags="${newlib_cflags} -DHAVE_FCNTL -DHAVE_RENAME"\
\t;;\
' "$f"
  echo "patched newlib/configure.host for myos"
}

patch_config_sub
patch_configure_host
patch_string_h_basename

echo "myos newlib patches applied"
