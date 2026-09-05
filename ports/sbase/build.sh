#!/usr/bin/env bash
# Cross-build all upstream sbase utilities with newlib + myos libgloss.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_sbase_is_current; then
  echo "sbase ELFs up to date"
  exit 0
fi

"$ROOT/ports/sbase/prepare.sh"
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

WORK="$ROOT/target/sbase-myos-build"
BINS_FILE="$ROOT/ports/sbase/bins.txt"
mapfile -t SBASE_BINS <"$BINS_FILE"

CPPFLAGS=(
  -DPREFIX=\"/bin\"
  -D_DEFAULT_SOURCE
  -D_GNU_SOURCE
  -D_NETBSD_SOURCE
  -D_BSD_SOURCE
  -D_XOPEN_SOURCE=700
  -D_FILE_OFFSET_BITS=64
  -I"$WORK"
  -I"$ROOT/ports/sbase"
  -include myos_compat.h
  -include sys/myos_extra.h
  -Wno-implicit-function-declaration
)

LIBUTIL_SRCS=(
  libutil/concat.c libutil/cp.c libutil/crypt.c libutil/confirm.c libutil/ealloc.c
  libutil/enmasse.c libutil/eprintf.c libutil/eregcomp.c libutil/estrtod.c
  libutil/fnck.c libutil/fshut.c libutil/getlines.c libutil/human.c libutil/linecmp.c
  libutil/md5.c libutil/memmem.c libutil/mkdirp.c libutil/mode.c libutil/parseoffset.c
  libutil/putword.c libutil/reallocarray.c libutil/recurse.c libutil/rm.c
  libutil/sha1.c libutil/sha224.c libutil/sha256.c libutil/sha384.c libutil/sha512.c
  libutil/sha512-224.c libutil/sha512-256.c libutil/strcasestr.c libutil/strlcat.c
  libutil/strlcpy.c libutil/strsep.c libutil/strnsubst.c libutil/strtonum.c
  libutil/unescape.c libutil/writeall.c
)

LIBUTF_SRCS=(
  libutf/fgetrune.c libutf/fputrune.c libutf/isalnumrune.c libutf/isalpharune.c
  libutf/isblankrune.c libutf/iscntrlrune.c libutf/isdigitrune.c libutf/isgraphrune.c
  libutf/isprintrune.c libutf/ispunctrune.c libutf/isspacerune.c libutf/istitlerune.c
  libutf/isxdigitrune.c libutf/lowerrune.c libutf/rune.c libutf/runetype.c
  libutf/upperrune.c libutf/utf.c libutf/utftorunestr.c
)

MAKE_SRCS=(
  make/defaults.c make/main.c make/parser.c make/posix.c make/rules.c
)

REGEX_SRCS=(
  posix/regcomp.c posix/regerror.c posix/regexec.c posix/regfree.c
  posix/collate.c posix/collcmp.c posix/fnmatch.c
)

compile() {
  local cc="$1"
  local inc="$2"
  local src="$3"
  local out="$4"
  "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" "${CPPFLAGS[@]}" -c "$src" -o "$out"
}

link_prog() {
  local out_name="$1"
  local arch="$2"
  shift 2
  local objs=("$@")
  local triple="${arch}-unknown-myos"
  local out="$ROOT/target/sbase-${out_name}-${arch}-unknown-none"
  local prefix="$ROOT/target/newlib-${arch}"
  local lib="$prefix/${triple}/lib"
  local ld="${triple}-ld"

  "$ld" -pie --no-dynamic-linker -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "${objs[@]}" -L"$lib" \
    --start-group -lc -lgloss -lg --end-group
}

build_arch() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local prefix="$ROOT/target/newlib-${arch}"
  local inc="$prefix/${triple}/include"
  local cc="${triple}-cc"
  local objdir="$ROOT/target/sbase-obj-${arch}"
  local manifest="$ROOT/target/sbase-manifest-${arch}.txt"
  local myos="$ROOT/ports/sbase"
  local built=0
  local failed=()

  rm -rf "$objdir"
  mkdir -p "$objdir"
  : >"$manifest"

  local util_objs=()
  local src base obj
  for src in "${LIBUTIL_SRCS[@]}"; do
    base="$(basename "$src" .c)"
    obj="$objdir/libutil-${base}.o"
    compile "$cc" "$inc" "$WORK/$src" "$obj"
    util_objs+=("$obj")
  done

  local utf_objs=()
  for src in "${LIBUTF_SRCS[@]}"; do
    base="$(basename "$src" .c)"
    obj="$objdir/libutf-${base}.o"
    compile "$cc" "$inc" "$WORK/$src" "$obj"
    utf_objs+=("$obj")
  done

  local regex_objs=()
  for src in "${REGEX_SRCS[@]}"; do
    base="$(basename "$src" .c)"
    obj="$objdir/regex-${base}.o"
    compile "$cc" "$inc" "$ROOT/target/newlib-src/newlib/libc/$src" "$obj"
    regex_objs+=("$obj")
  done

  local libs=("${util_objs[@]}" "${utf_objs[@]}" "${regex_objs[@]}")

  local extra=()
  if [[ "$arch" == "aarch64" ]]; then
    compile "$cc" "$inc" "$myos/trunctfdf2.c" "$objdir/trunctfdf2.o"
    extra=("$objdir/trunctfdf2.o")
  elif [[ "$arch" == "riscv64" ]]; then
    compile "$cc" "$inc" "$myos/riscv64-softfloat.c" "$objdir/riscv64-softfloat.o"
    extra=("$objdir/riscv64-softfloat.o")
  fi

  local make_objs=()
  for src in "${MAKE_SRCS[@]}"; do
    base="$(basename "$src" .c)"
    obj="$objdir/make-${base}.o"
    compile "$cc" "$inc" "$WORK/$src" "$obj"
    make_objs+=("$obj")
  done

  try_build() {
    local name="$1"
    local src_rel="$2"
    shift 2
    local extra_objs=("$@")
    local out="$ROOT/target/sbase-${name}-${arch}-unknown-none"
    local obj="$objdir/prog-${name}.o"
    echo "==> sbase-${name} ($triple)"
    if ! compile "$cc" "$inc" "$WORK/$src_rel" "$obj"; then
      failed+=("$name:compile")
      return 1
    fi
    if ! link_prog "$name" "$arch" "${libs[@]}" "${extra_objs[@]}" "$obj" "${extra[@]}"; then
      failed+=("$name:link")
      rm -f "$out" "$obj"
      return 1
    fi
    echo "${name}:${out}" >>"$manifest"
    built=$((built + 1))
    return 0
  }

  for name in "${SBASE_BINS[@]}"; do
    case "$name" in
      make)
        echo "==> sbase-make ($triple)"
        local make_libs=()
        for o in "${libs[@]}"; do
          [[ "$o" == *libutil-ealloc.o ]] && continue
          make_libs+=("$o")
        done
        if link_prog make "$arch" "${make_libs[@]}" "${make_objs[@]}" "${extra[@]}"; then
          echo "make:$ROOT/target/sbase-make-${arch}-unknown-none" >>"$manifest"
          built=$((built + 1))
        else
          failed+=("make:link")
        fi
        ;;
      bc)
        if [[ -f "$WORK/bc.c" ]]; then
          try_build bc bc.c || true
        else
          failed+=("bc:no-bc.c")
        fi
        ;;
      getconf)
        if [[ -f "$WORK/getconf.h" ]]; then
          try_build getconf getconf.c || true
        else
          failed+=("getconf:no-header")
        fi
        ;;
      *)
        try_build "$name" "${name}.c" || true
        ;;
    esac
  done

  echo "sbase ${arch}: built ${built}/$((${#SBASE_BINS[@]})) (${#failed[@]} failed)"
  if ((${#failed[@]} > 0)); then
    printf '  failed: %s\n' "${failed[@]}" >&2
  fi
  if ((built == 0)); then
    echo "error: no sbase ELFs built for ${arch}" >&2
    exit 1
  fi
}

for arch in x86_64 aarch64 riscv64; do
  build_arch "$arch"
done

echo "$(myos_sbase_version_hash)" >"$MYOS_SBASE_VERSION"
