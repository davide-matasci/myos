#!/usr/bin/env bash
# Cross-build TinyCC for all *-unknown-myos triples (target codegen, not host-only).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=ports/tcc/versions.env
source "$HERE/versions.env"

is_elf() {
  local f="$1"
  local mag
  [[ -f "$f" && -s "$f" ]] || return 1
  mag="$(od -An -N4 -tx1 "$f" 2>/dev/null | tr -d ' \n')"
  [[ "$mag" == "7f454c46" ]]
}

require_elf() {
  local arch="$1"
  local out="$ROOT/target/tcc-${arch}-unknown-myos"
  if ! is_elf "$out"; then
    echo "error: tcc ELF missing for ${arch} at ${out}" >&2
    return 1
  fi
}

libtcc1_out() {
  echo "$ROOT/target/libtcc1-${1}-unknown-myos.a"
}

require_libtcc1() {
  local arch="$1"
  local f
  f="$(libtcc1_out "$arch")"
  if [[ ! -f "$f" || ! -s "$f" ]]; then
    echo "error: libtcc1.a missing for ${arch} at ${f}" >&2
    return 1
  fi
}

install_libtcc1() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local dest="$ROOT/target/newlib-${arch}/${triple}/lib/libtcc1.a"
  mkdir -p "$(dirname "$dest")"
  cp "$(libtcc1_out "$arch")" "$dest"
}

pack_aliases() {
  local arch triple out alias
  for arch in x86_64 aarch64 riscv64; do
    require_elf "$arch" || exit 1
    triple="${arch}-unknown-myos"
    out="$ROOT/target/tcc-${triple}"
    alias="$ROOT/target/coreutils-tcc-${triple}"
    cp "$out" "$alias"
    require_libtcc1 "$arch" || exit 1
    install_libtcc1 "$arch"
  done
}

all_elfs_ok() {
  local arch
  for arch in x86_64 aarch64 riscv64; do
    is_elf "$ROOT/target/tcc-${arch}-unknown-myos" || return 1
  done
}

all_libtcc1_ok() {
  local arch f
  for arch in x86_64 aarch64 riscv64; do
    f="$(libtcc1_out "$arch")"
    [[ -f "$f" && -s "$f" ]] || return 1
  done
}

if myos_tcc_is_current && all_elfs_ok && all_libtcc1_ok; then
  echo "tcc ELFs + libtcc1.a up to date"
  pack_aliases
  exit 0
fi
if myos_tcc_is_current && ! all_elfs_ok; then
  echo "error: tcc stamp current but an arch ELF is missing; rebuilding" >&2
fi

"$ROOT/ports/tcc/prepare.sh"
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

WORK="$ROOT/target/tcc-myos-build"
MYOS="$ROOT/ports/tcc"

tcc_target_def() {
  case "$1" in
    x86_64) echo "-DTCC_TARGET_X86_64" ;;
    aarch64) echo "-DTCC_TARGET_ARM64" ;;
    riscv64) echo "-DTCC_TARGET_RISCV64" ;;
    *) echo "error: unknown arch $1" >&2; return 1 ;;
  esac
}

build_arch() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local prefix="$ROOT/target/newlib-${arch}"
  local inc="$prefix/${triple}/include"
  local lib="$prefix/${triple}/lib"
  local cc="${triple}-cc"
  local ld="${triple}-ld"
  local objdir="$ROOT/target/tcc-obj-${arch}"
  local out="$ROOT/target/tcc-${triple}"
  local extra=()
  local target_def
  local libarm="$WORK/lib/lib-arm64.c"
  local rvsoft="$ROOT/ports/sbase/riscv64-softfloat.c"

  target_def="$(tcc_target_def "$arch")"
  rm -rf "$objdir"
  mkdir -p "$objdir"
  rm -f "$out"

  echo "==> tcc ($triple)"
  "$cc" -ffreestanding -fPIC -O2 \
    -isystem "$inc" \
    -I"$WORK" \
    -I"$MYOS" \
    -I"$ROOT/toolchain/newlib/libgloss/myos" \
    -include sys/myos_extra.h \
    -DONE_SOURCE=1 \
    $target_def \
    -DCONFIG_TCC_STATIC=1 \
    -D_DEFAULT_SOURCE \
    -D_GNU_SOURCE \
    -Wno-implicit-function-declaration \
    -Wno-unused-parameter \
    -Wno-unused-variable \
    -Wno-pointer-sign \
    -c "$WORK/tcc.c" -o "$objdir/tcc.o"

  # No -lm: tcc only needs ldexp/strtold for constant folding.
  "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
    -c "$MYOS/host_math.c" -o "$objdir/host_math.o"
  extra+=("$objdir/host_math.o")

  # clang --target=*-unknown-none uses IEEE-128 long double and emits
  # compiler-rt __*tf* helpers. TinyCC's lib/lib-arm64.c is the canonical
  # software implementation (also used for riscv64 libtcc1 upstream).
  if [[ "$arch" == "aarch64" || "$arch" == "riscv64" ]]; then
    if [[ ! -f "$libarm" ]]; then
      echo "error: TinyCC IEEE-128 helpers missing at ${libarm}" >&2
      exit 1
    fi
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
      -c "$libarm" -o "$objdir/lib-arm64.o"
    extra+=("$objdir/lib-arm64.o")
  fi

  # riscv64-unknown-none is soft-float; libc strtod may need df helpers.
  # Rename overlapping __trunctfdf2 so lib-arm64.c owns the IEEE-128 trunc.
  if [[ "$arch" == "riscv64" ]]; then
    if [[ ! -f "$rvsoft" ]]; then
      echo "error: riscv64 softfloat helpers missing at ${rvsoft}" >&2
      exit 1
    fi
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
      -D__trunctfdf2=__myos_tcc_unused_trunctfdf2 \
      -c "$rvsoft" -o "$objdir/riscv64-softfloat.o"
    extra+=("$objdir/riscv64-softfloat.o")
  fi

  "$ld" -pie --no-dynamic-linker --gc-sections -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "$objdir/tcc.o" "${extra[@]}" -L"$lib" \
    --start-group -lc -lgloss -lg --end-group

  "${triple}-strip" -s "$out" 2>/dev/null || strip -s "$out" 2>/dev/null || true

  if ! is_elf "$out"; then
    echo "error: tcc ELF missing for ${arch} at ${out}" >&2
    exit 1
  fi
  echo "tcc -> $out ($(du -h "$out" | awk '{print $1}'))"
  build_libtcc1 "$arch"
}

# Guest compiler runtime (TinyCC lib/), not libc. Matches tcc's lib/Makefile
# unix/cross set with CONFIG_bcheck=0 CONFIG_backtrace=0. Compiled with the
# myos cross cc like the rest of the port (upstream default is tcc itself).
build_libtcc1() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local prefix="$ROOT/target/newlib-${arch}"
  local inc="$prefix/${triple}/include"
  local cc="${triple}-cc"
  local src="$WORK/lib"
  local odir="$ROOT/target/tcc-libtcc1-obj-${arch}"
  local out
  out="$(libtcc1_out "$arch")"
  local srcs=()
  local f obj objs=()

  rm -rf "$odir"
  mkdir -p "$odir"
  rm -f "$out"

  srcs+=(stdatomic.c atomic.S builtin.c dsohandle.c)
  case "$arch" in
    x86_64)
      srcs+=(libtcc1.c alloca.S va_list.c)
      ;;
    aarch64|riscv64)
      srcs+=(lib-arm64.c armflush.c)
      ;;
    *)
      echo "error: unknown arch $arch" >&2
      return 1
      ;;
  esac

  echo "==> libtcc1.a ($triple)"
  for f in "${srcs[@]}"; do
    if [[ ! -f "$src/$f" ]]; then
      echo "error: TinyCC lib/$f missing at ${src}/${f}" >&2
      return 1
    fi
    obj="$odir/${f%.*}.o"
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" -I"$WORK" -I"$src"       -c "$src/$f" -o "$obj"
    objs+=("$obj")
  done
  ar rcs "$out" "${objs[@]}"
  install_libtcc1 "$arch"
  echo "libtcc1.a -> $out ($(du -h "$out" | awk '{print $1}'))"
}

for arch in x86_64 aarch64 riscv64; do
  build_arch "$arch"
done

missing=0
for arch in x86_64 aarch64 riscv64; do
  if ! require_elf "$arch"; then
    missing=1
  fi
done
if ((missing != 0)); then
  exit 1
fi

pack_aliases
echo "$(myos_tcc_version_hash)" >"$MYOS_TCC_VERSION"
