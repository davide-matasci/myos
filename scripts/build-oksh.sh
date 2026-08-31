#!/usr/bin/env bash
# Cross-build portable OpenBSD ksh (oksh) with newlib + myos libgloss.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_oksh_is_current; then
  echo "oksh ELFs up to date"
  exit 0
fi

"$ROOT/scripts/prepare-oksh-myos.sh"
"$ROOT/scripts/build-newlib.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

WORK="$ROOT/target/oksh-myos-build"
MYOS="$ROOT/scripts/oksh-myos"

# config.h requires EMACS or VI. Keep EMACS (not VI) for size. emacs.c
# supplies x_* symbols; main.myos.patch still skips x_init (cooked stdin,
# no raw tty). confstr.c is oksh's portable fallback (HAVE_CONFSTR off).
# c_ulimit.c is the myos stub.
OKSH_SRCS=(
  alloc.c asprintf.c c_ksh.c c_sh.c c_test.c c_ulimit.c edit.c emacs.c
  eval.c exec.c expr.c history.c io.c jobs.c lex.c mail.c
  main.c misc.c path.c shf.c syn.c table.c trap.c tree.c tty.c var.c
  version.c reallocarray.c siglist.c signame.c confstr.c
  strlcat.c strlcpy.c strtonum.c unvis.c vis.c issetugid.c
)

CPPFLAGS=(
  -D_DEFAULT_SOURCE
  -D_GNU_SOURCE
  -D_BSD_SOURCE
  -DSMALL
  -DEMACS
  -D_PATH_DEFPATH=\"/:/s:/c\"
  -D_PATH_BSHELL=\"/sh\"
  -D_PATH_STDPATH=\"/:/s:/c\"
  -D_PW_NAME_LEN=32
  -I"$WORK"
  -I"$MYOS"
  -I"$ROOT/newlib/libgloss/myos"
  -include myos_compat.h
)

link_prog() {
  local arch="$1"
  shift
  local objs=("$@")
  local triple="${arch}-unknown-myos"
  local out="$ROOT/target/oksh-${arch}-unknown-none"
  local prefix="$ROOT/target/newlib-${arch}"
  local lib="$prefix/${triple}/lib"
  local ld="${triple}-ld"

  "$ld" -pie --no-dynamic-linker --gc-sections -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "${objs[@]}" -L"$lib" \
    --start-group -lc -lgloss -lg --end-group
  "${triple}-strip" -s "$out" 2>/dev/null || strip -s "$out" 2>/dev/null || true
  echo "oksh -> $out"
  if command -v llvm-size >/dev/null 2>&1; then
    llvm-size "$out" || true
  elif command -v size >/dev/null 2>&1; then
    size "$out" || true
  fi
}

build_arch() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local prefix="$ROOT/target/newlib-${arch}"
  local inc="$prefix/${triple}/include"
  local cc="${triple}-cc"
  local objdir="$ROOT/target/oksh-obj-${arch}"
  local extra=()
  local src base obj
  local objs=()

  rm -rf "$objdir"
  mkdir -p "$objdir"

  echo "==> oksh ($triple)"
  for src in "${OKSH_SRCS[@]}"; do
    base="$(basename "$src" .c)"
    obj="$objdir/${base}.o"
    "$cc" -ffreestanding -fPIC -O2 -std=gnu99 \
      -ffunction-sections -fdata-sections \
      -isystem "$inc" "${CPPFLAGS[@]}" \
      -c "$WORK/$src" -o "$obj"
    objs+=("$obj")
  done

  "$cc" -ffreestanding -fPIC -O2 -std=gnu99 \
    -ffunction-sections -fdata-sections \
    -isystem "$inc" "${CPPFLAGS[@]}" \
    -c "$MYOS/posix_nops.c" -o "$objdir/posix_nops.o"
  objs+=("$objdir/posix_nops.o")

  if [[ "$arch" == "aarch64" ]]; then
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
      -c "$ROOT/scripts/sbase-myos/trunctfdf2.c" -o "$objdir/trunctfdf2.o"
    extra+=("$objdir/trunctfdf2.o")
  elif [[ "$arch" == "riscv64" ]]; then
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
      -c "$ROOT/scripts/sbase-myos/riscv64-softfloat.c" -o "$objdir/riscv64-softfloat.o"
    extra+=("$objdir/riscv64-softfloat.o")
  fi

  link_prog "$arch" "${objs[@]}" "${extra[@]}"
}

for arch in x86_64 aarch64 riscv64; do
  build_arch "$arch"
done

echo "$(myos_oksh_version_hash)" >"$MYOS_OKSH_VERSION"
