#!/usr/bin/env bash
# Cross-build Git (Phase-1 local porcelain) with newlib + libgloss + zlib.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=ports/git/versions.env
source "$HERE/versions.env"

is_elf() {
  local f="$1" mag
  [[ -f "$f" && -s "$f" ]] || return 1
  mag="$(od -An -N4 -tx1 "$f" 2>/dev/null | tr -d ' \n')"
  [[ "$mag" == "7f454c46" ]]
}

if myos_git_is_current; then
  echo "git ELFs up to date"
  exit 0
fi

"$ROOT/ports/git/prepare.sh"
"$ROOT/ports/zlib/build.sh"
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

WORK="$ROOT/target/git-myos-build"
MYOS="$ROOT/ports/git"

build_arch() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local out="$ROOT/target/git-${arch}-unknown-none"
  local objdir="$ROOT/target/git-obj-${arch}"
  local stub_obj="$objdir/myos_stubs.o"
  local cc="$MYOS/myos-git-cc.sh"
  local prefix="$ROOT/target/newlib-${arch}"
  local inc="$prefix/${triple}/include"
  local extra_obj=()
  local make_jobs="${MYOS_GIT_JOBS:-$(nproc 2>/dev/null || echo 4)}"

  echo "==> git ($triple)"
  rm -rf "$objdir"
  mkdir -p "$objdir"
  rm -f "$out"

  # Soft-float helpers for aarch64/riscv64 (same as vim/oksh).
  if [[ "$arch" == "aarch64" ]]; then
    "${triple}-cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
      -c "$ROOT/ports/sbase/trunctfdf2.c" -o "$objdir/trunctfdf2.o"
    extra_obj+=("$objdir/trunctfdf2.o")
  elif [[ "$arch" == "riscv64" ]]; then
    "${triple}-cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
      -c "$ROOT/ports/sbase/riscv64-softfloat.c" -o "$objdir/riscv64-softfloat.o"
    extra_obj+=("$objdir/riscv64-softfloat.o")
    "${triple}-cc" -ffreestanding -fPIC -O2 -isystem "$inc" \
      -c "$MYOS/riscv64-sf-arith.c" -o "$objdir/riscv64-sf-arith.o"
    extra_obj+=("$objdir/riscv64-sf-arith.o")
  fi

  MYOS_GIT_ARCH="$arch" MYOS_GIT_ROOT="$ROOT" \
    "$cc" -c "$MYOS/myos_stubs.c" -o "$stub_obj"
  extra_obj+=("$stub_obj")

  # Clean prior arch objects inside the shared work tree.
  make -C "$WORK" clean >/dev/null 2>&1 || true
  # Drop host-built leftovers that confuse cross rebuilds.
  find "$WORK" -name '*.o' -o -name '*.a' -o -name 'git' \
    | while read -r f; do rm -f "$f"; done

  export MYOS_GIT_ARCH="$arch"
  export MYOS_GIT_ROOT="$ROOT"
  export MYOS_GIT_EXTRA_OBJ="${extra_obj[*]}"

  # Build only the main multi-call binary (no dashed builtins / scripts).
  make -C "$WORK" -j"$make_jobs" \
    SHELL=/bin/bash \
    SHELL_PATH=/bin/bash \
    CC="$cc" \
    AR=ar \
    RANLIB=ranlib \
    CFLAGS="-ffreestanding -fPIC -O2" \
    LDFLAGS="" \
    ZLIB_PATH="$ROOT/target/zlib-${arch}" \
    prefix=/usr \
    git

  if [[ ! -f "$WORK/git" ]]; then
    echo "error: git binary missing after make ($arch)" >&2
    exit 1
  fi
  cp "$WORK/git" "$out"
  # Pack alias for ci-build.tar `coreutils-*` glob when workflow cannot list git-*.
  cp "$out" "$ROOT/target/coreutils-git-${arch}-unknown-none"
  if ! is_elf "$out"; then
    echo "error: git ELF missing for ${arch} at ${out}" >&2
    exit 1
  fi
  echo "git -> $out ($(du -h "$out" | awk '{print $1}'))"
  if command -v llvm-size >/dev/null 2>&1; then
    llvm-size "$out" || true
  elif command -v size >/dev/null 2>&1; then
    size "$out" || true
  fi
}

# Default: all arches (CI). MYOS_GIT_ARCHES=x86_64 for a faster local smoke.
ARCHES="${MYOS_GIT_ARCHES:-x86_64 aarch64 riscv64}"
# shellcheck disable=SC2086
for arch in $ARCHES; do
  build_arch "$arch"
done

# Stamp only when all three default arches exist (CI expectation).
if myos_git_elfs_present; then
  echo "$(myos_git_version_hash)" >"$MYOS_GIT_VERSION"
else
  echo "warning: not all arch ELFs present; stamp not written (partial build)" >&2
fi
