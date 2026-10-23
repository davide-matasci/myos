#!/usr/bin/env bash
# The C smoke programs of the boot tests (user/c, the `c-smokes` port):
# c-hello and socket_smoke (build-c-hello.sh, the kernel embeds c-hello), and
# the ones below (MYOS_C_SMOKES), each a static PIE against newlib +
# libgloss/myos for the three arches: target/<name>-<arch>-unknown-none.
# With `mlibc` as the argument, all of them (hello and socket_smoke too)
# against mlibc instead (user/c-mlibc, the `c-smokes-mlibc` port):
# target/<name>-mlibc-<arch>-unknown-none. The same sources, so a test
# that passes with both libcs covers the kernel's interface through either.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT/scripts/myos-c-userspace-lib.sh"

libc="${1:-newlib}"
case "$libc" in
  newlib)
    if myos_c_smokes_is_current; then
      echo "c smokes up to date"
      exit 0
    fi
    "$ROOT/scripts/build-c-hello.sh"
    "$ROOT/toolchain/newlib/build.sh"
    export PATH="$ROOT/target/newlib-bin:$PATH"
    smokes=("${MYOS_C_SMOKES[@]}")
    suffix="" ;;
  mlibc)
    if myos_c_smokes_mlibc_is_current; then
      echo "c smokes (mlibc) up to date"
      exit 0
    fi
    "$ROOT/toolchain/mlibc/build.sh"
    smokes=("c-hello user/c/hello.c" "c-socket_smoke user/c/socket_smoke.c" "${MYOS_C_SMOKES[@]}")
    suffix="-mlibc" ;;
  *) echo "usage: $0 [newlib|mlibc]" >&2; exit 2 ;;
esac
myos_ensure_llvm_bin

for arch in x86_64 aarch64 riscv64; do
  triple="${arch}-unknown-myos"
  prefix="$ROOT/target/newlib-${arch}"
  inc="$prefix/${triple}/include"
  lib="$prefix/${triple}/lib"
  cc="${triple}-cc"
  # newlib's printf wants the long-double and soft-float helpers these
  # arches lack in the shim (the sbase port carries them).
  extra=()
  if [[ "$libc" == newlib && "$arch" == "aarch64" ]]; then
    tf="$ROOT/target/c-smokes-${arch}-trunctfdf2.o"
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" -c "$ROOT/ports/sbase/trunctfdf2.c" -o "$tf"
    extra+=("$tf")
  elif [[ "$libc" == newlib && "$arch" == "riscv64" ]]; then
    sf="$ROOT/target/c-smokes-${arch}-softfloat.o"
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" -c "$ROOT/ports/sbase/riscv64-softfloat.c" -o "$sf"
    extra+=("$sf" "$(myos_riscv64_softfloat)")
  fi
  for entry in "${smokes[@]}"; do
    name="${entry%% *}"
    src="${entry#* }"
    out="$ROOT/target/${name}${suffix}-${arch}-unknown-none"
    obj="$ROOT/target/${name}${suffix}-${arch}.o"
    echo "==> ${name} ($triple / $libc)"
    if [[ "$libc" == newlib ]]; then
      "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" -c "$ROOT/$src" -o "$obj"
      ld.lld -pie --no-dynamic-linker -o "$out" \
        --entry=_start -z max-page-size=4096 \
        "$lib/crt0.o" "$obj" "${extra[@]+"${extra[@]}"}" -L"$lib" \
        --start-group -lc -lgloss -lg --end-group
    else
      # shellcheck disable=SC2046
      clang $(myos_mlibc_cflags "$arch") -O2 -c "$ROOT/$src" -o "$obj"
      myos_mlibc_link "$arch" "$out" "$obj"
    fi
  done
done

if [[ "$libc" == newlib ]]; then
  myos_c_smokes_version_hash > "$MYOS_C_SMOKES_VERSION"
else
  myos_c_smokes_mlibc_version_hash > "$MYOS_C_SMOKES_MLIBC_VERSION"
fi
echo "c smokes ($libc) -> target/{$(printf '%s\n' "${smokes[@]}" | cut -d' ' -f1 | paste -sd,)}${suffix}-<arch>-unknown-none"
