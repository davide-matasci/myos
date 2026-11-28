#!/bin/sh
# Cross-build ubase getty, login and df with newlib + myos libgloss.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
. "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_ubase_is_current; then
  echo "ubase ELFs up to date"
  exit 0
fi

# On myos (scripts/self-host.sh) the tree is the myos-src package's,
# prepared where the package was built.
myos_self_host || "$ROOT/ports/ubase/prepare.sh"
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

WORK="$ROOT/target/ubase-myos-build"
BINS_FILE="$ROOT/ports/ubase/bins.txt"
MYOS="$ROOT/ports/ubase"

# A string, split where it is used: no flag holds a space.
CPPFLAGS="
  -D_DEFAULT_SOURCE
  -D_GNU_SOURCE
  -D_BSD_SOURCE
  -D_XOPEN_SOURCE=700
  -I$WORK
  -I$MYOS
  -I$ROOT/toolchain/newlib/libgloss/myos
  -include myos_compat.h
  -include sys/myos_extra.h
  -Wno-implicit-function-declaration
"

LIBUTIL_SRCS="
  libutil/eprintf.c
  libutil/ealloc.c
  libutil/strlcpy.c
  libutil/strlcat.c
"

# compile CC INC SRC OUT
compile() {
  # shellcheck disable=SC2086
  "$1" -ffreestanding -fPIC -O2 -isystem "$2" $CPPFLAGS -c "$3" -o "$4"
}

# True iff path exists and starts with ELF magic. Used so a failed link
# (undefined symbol) cannot be counted as built when strip's || true hid ld.
is_elf() (
  f="$1"
  { [ -f "$f" ] && [ -s "$f" ]; } || return 1
  # dd, not od -N: sbase's od, which a build on myos runs, has no -N.
  [ "$(dd if="$f" bs=1 skip=1 count=3 2>/dev/null)" = ELF ]
)

link_prog() (
  out_name="$1"
  arch="$2"
  shift 2
  triple="${arch}-unknown-myos"
  out="$ROOT/target/ubase-${out_name}-${arch}-unknown-none"
  prefix="$ROOT/target/newlib-${arch}"
  lib="$prefix/${triple}/lib"
  ld="${triple}-ld"

  rm -f "$out"
  # Called from `if ! link_prog`; set -e is disabled, so check ld explicitly.
  if ! "$ld" -pie --no-dynamic-linker --gc-sections -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "$@" -L"$lib" \
    --start-group -lc -lgloss -lg --end-group
  then
    rm -f "$out"
    return 1
  fi
  if ! is_elf "$out"; then
    rm -f "$out"
    echo "error: ubase ELF missing for ${out_name} at ${out}" >&2
    return 1
  fi
  "${triple}-strip" -s "$out" 2>/dev/null || strip -s "$out" 2>/dev/null || true
  if ! is_elf "$out"; then
    rm -f "$out"
    echo "error: ubase ELF missing for ${out_name} at ${out}" >&2
    return 1
  fi
  return 0
)

build_arch() (
  arch="$1"
  triple="${arch}-unknown-myos"
  prefix="$ROOT/target/newlib-${arch}"
  inc="$prefix/${triple}/include"
  cc="${triple}-cc"
  objdir="$ROOT/target/ubase-obj-${arch}"
  manifest="$ROOT/target/ubase-manifest-${arch}.txt"
  built=0
  failed="" nfailed=0
  extra=""

  rm -rf "$objdir"
  mkdir -p "$objdir"
  : >"$manifest"

  # The libutil objects are the positional parameters.
  set --
  for src in $LIBUTIL_SRCS; do
    base="$(basename "$src" .c)"
    obj="$objdir/libutil-${base}.o"
    compile "$cc" "$inc" "$WORK/$src" "$obj"
    set -- "$@" "$obj"
  done

  compile "$cc" "$inc" "$MYOS/pw_check.c" "$objdir/pw_check.o"
  set -- "$@" "$objdir/pw_check.o"

  if [ "$arch" = "aarch64" ] || [ "$arch" = "riscv64" ]; then
    if [ -f "$ROOT/ports/sbase/trunctfdf2.c" ]; then
      compile "$cc" "$inc" "$ROOT/ports/sbase/trunctfdf2.c" "$objdir/trunctfdf2.o"
      extra="$objdir/trunctfdf2.o"
    fi
  fi

  expected=0
  while IFS= read -r name; do
    [ -n "$name" ] || continue
    expected=$((expected + 1))
  done <"$BINS_FILE"

  while IFS= read -r name; do
    [ -n "$name" ] || continue
    obj="$objdir/prog-${name}.o"
    out="$ROOT/target/ubase-${name}-${arch}-unknown-none"
    echo "==> ubase-${name} ($triple)"
    if ! compile "$cc" "$inc" "$WORK/${name}.c" "$obj"; then
      failed="$failed $name:compile" nfailed=$((nfailed + 1))
      rm -f "$out"
      continue
    fi
    if ! link_prog "$name" "$arch" "$@" "$obj" ${extra:+"$extra"}; then
      failed="$failed $name:link" nfailed=$((nfailed + 1))
      rm -f "$out"
      continue
    fi
    if ! is_elf "$out"; then
      failed="$failed $name:missing-elf" nfailed=$((nfailed + 1))
      rm -f "$out"
      echo "error: ubase ELF missing for ${name} at ${out}" >&2
      continue
    fi
    # Manifest only for ELFs that actually exist.
    echo "${name}:${out}" >>"$manifest"
    built=$((built + 1))
  done <"$BINS_FILE"

  echo "ubase ${arch}: built ${built}/${expected} ($nfailed failed)"
  if [ "$nfailed" -gt 0 ]; then
    # shellcheck disable=SC2086
    printf '  failed: %s\n' $failed >&2
  fi
  while IFS= read -r name; do
    [ -n "$name" ] || continue
    out="$ROOT/target/ubase-${name}-${arch}-unknown-none"
    if ! is_elf "$out"; then
      echo "error: ubase ELF missing for ${name} at ${out}" >&2
      failed="$failed $name:missing-elf" nfailed=$((nfailed + 1))
    fi
  done <"$BINS_FILE"
  if [ "$nfailed" -gt 0 ] || [ "$built" -ne "$expected" ]; then
    echo "error: not all ubase ELFs built for ${arch} (${built}/${expected})" >&2
    exit 1
  fi
)

for arch in $MYOS_ARCHES; do
  build_arch "$arch"
done

# Stamp only after every bins.txt ELF exists on every arch.
missing=0
for arch in $MYOS_ARCHES; do
  while IFS= read -r name; do
    [ -n "$name" ] || continue
    out="$ROOT/target/ubase-${name}-${arch}-unknown-none"
    if ! is_elf "$out"; then
      echo "error: ubase ELF missing for ${name} at ${out}" >&2
      missing=1
    fi
  done <"$BINS_FILE"
done
if [ "$missing" -ne 0 ]; then
  exit 1
fi

myos_self_host || echo "$(myos_ubase_version_hash)" >"$MYOS_UBASE_VERSION"
