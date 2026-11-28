#!/bin/sh
# Cross-build portable OpenBSD ksh (oksh) with newlib + myos libgloss.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
. "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_oksh_is_current; then
  echo "oksh ELFs up to date"
  exit 0
fi

# On myos (scripts/self-host.sh) the tree is the myos-src package's,
# prepared where the package was built.
myos_self_host || "$ROOT/ports/oksh/prepare.sh"
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

WORK="$ROOT/target/oksh-myos-build"
MYOS="$ROOT/ports/oksh"

# config.h requires EMACS or VI. Keep EMACS (not VI) for size. emacs.c
# supplies x_* symbols; main.myos.patch runs x_init() so the emacs line
# editor drives a raw-mode console tty (raw edit, arrows navigate history /
# cursor). confstr.c is oksh's portable fallback (HAVE_CONFSTR off).
# c_ulimit.c is the myos stub.
OKSH_SRCS="
  alloc.c asprintf.c c_ksh.c c_sh.c c_test.c c_ulimit.c edit.c emacs.c
  eval.c exec.c expr.c history.c io.c jobs.c lex.c mail.c
  main.c misc.c path.c shf.c syn.c table.c trap.c tree.c tty.c var.c
  version.c reallocarray.c siglist.c signame.c confstr.c
  strlcat.c strlcpy.c strtonum.c unvis.c vis.c issetugid.c
"

# CPPFLAGS: the positional parameters, which build_arch is given (a string
# split where it is used could not hold the quotes of the paths).
set -- \
  -D_DEFAULT_SOURCE \
  -D_GNU_SOURCE \
  -D_BSD_SOURCE \
  -DEMACS \
  -D_PATH_DEFPATH=\"/bin/sbase:/bin/coreutils:/bin/ubase:/bin/custom:/bin/tcc:/bin/std:/bin/etc\" \
  -D_PATH_BSHELL=\"/bin/custom/sh\" \
  -D_PATH_STDPATH=\"/bin/sbase:/bin/coreutils:/bin/ubase:/bin/custom:/bin/tcc:/bin/std:/bin/etc\" \
  -D_PW_NAME_LEN=32 \
  -I"$WORK" \
  -I"$MYOS" \
  -I"$ROOT/toolchain/newlib/libgloss/myos" \
  -include myos_compat.h

link_prog() (
  arch="$1"
  shift
  triple="${arch}-unknown-myos"
  out="$ROOT/target/oksh-${arch}-unknown-none"
  prefix="$ROOT/target/newlib-${arch}"
  lib="$prefix/${triple}/lib"
  ld="${triple}-ld"

  "$ld" -pie --no-dynamic-linker --gc-sections -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "$@" -L"$lib" \
    --start-group -lc -lgloss -lg --end-group
  "${triple}-strip" -s "$out" 2>/dev/null || strip -s "$out" 2>/dev/null || true
  echo "oksh -> $out"
  if command -v llvm-size >/dev/null 2>&1; then
    llvm-size "$out" || true
  elif command -v size >/dev/null 2>&1; then
    size "$out" || true
  fi
)

# build_arch ARCH CPPFLAGS...
build_arch() (
  arch="$1"
  shift
  triple="${arch}-unknown-myos"
  prefix="$ROOT/target/newlib-${arch}"
  inc="$prefix/${triple}/include"
  cc="${triple}-cc"
  objdir="$ROOT/target/oksh-obj-${arch}"
  extra=""
  objs=""

  rm -rf "$objdir"
  mkdir -p "$objdir"

  echo "==> oksh ($triple)"
  for src in $OKSH_SRCS; do
    base="$(basename "$src" .c)"
    obj="$objdir/${base}.o"
    "$cc" -ffreestanding -fPIC -O2 -std=gnu99 \
      -ffunction-sections -fdata-sections \
      -isystem "$inc" "$@" \
      -c "$WORK/$src" -o "$obj"
    objs="$objs $obj"
  done

  if [ "$arch" = "aarch64" ] || [ "$arch" = "riscv64" ]; then
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
      -c "$ROOT/ports/sbase/trunctfdf2.c" -o "$objdir/trunctfdf2.o"
    extra="$objdir/trunctfdf2.o"
  fi

  # shellcheck disable=SC2086
  link_prog "$arch" $objs ${extra:+"$extra"}
)

for arch in $MYOS_ARCHES; do
  build_arch "$arch" "$@"
done

myos_self_host || echo "$(myos_oksh_version_hash)" >"$MYOS_OKSH_VERSION"
