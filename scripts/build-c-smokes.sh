#!/usr/bin/env bash
# The C smoke programs of the boot tests (user/c, the `c-smokes` port):
# c-hello and socket_smoke (build-c-hello.sh, the kernel embeds c-hello), and
# the ones below (MYOS_C_SMOKES), each a static PIE against newlib +
# libgloss/myos for the three arches: target/<name>-<arch>-unknown-none.
# With `mlibc` as the argument, all of them (hello and socket_smoke too)
# against mlibc instead (user/c-mlibc, the `c-smokes-mlibc` port):
# target/<name>-mlibc-<arch>-unknown-none. The same sources, so a test
# that passes with both libcs covers the kernel's interface through either.
# And the C++ runtimes' smoke (user/c-mlibc/cxx_smoke.cpp) over libc++
# (toolchain/libcxx), which ships no program of its own, and the shared
# objects' (user/c-mlibc/dl_smoke.c: a dynamic program and two objects).
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
    "$ROOT/toolchain/libcxx/build.sh"
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
  # newlib's printf wants the long-double helpers these arches lack in the
  # shim (the sbase port carries them).
  extra=()
  if [[ "$libc" == newlib && ( "$arch" == "aarch64" || "$arch" == "riscv64" ) ]]; then
    tf="$ROOT/target/c-smokes-${arch}-trunctfdf2.o"
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" -c "$ROOT/ports/sbase/trunctfdf2.c" -o "$tf"
    extra+=("$tf")
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
  if [[ "$libc" == mlibc ]]; then
    echo "==> cxx-smoke ($triple / mlibc, libc++)"
    obj="$ROOT/target/cxx-smoke-mlibc-${arch}.o"
    # shellcheck disable=SC2046
    clang++ $(myos_libcxx_cxxflags "$arch") -O2 -std=c++23 -c "$ROOT/user/c-mlibc/cxx_smoke.cpp" -o "$obj"
    myos_libcxx_link "$arch" "$ROOT/target/cxx-smoke-mlibc-${arch}-unknown-none" "$obj"
    # The shared objects' smoke (user/c-mlibc/dl_smoke.c): a program
    # against the shared mlibc, the object it names (found beside it,
    # $ORIGIN) and the one it loads with dlopen.
    echo "==> dl-smoke ($triple / mlibc, shared)"
    for src in dl_lib dl_plugin dl_smoke; do
      # shellcheck disable=SC2046
      clang $(myos_mlibc_cflags "$arch") -O2 -c "$ROOT/user/c-mlibc/$src.c" -o "$ROOT/target/dl-${src}-mlibc-${arch}.o"
    done
    lib="$ROOT/target/dl-smoke-lib-mlibc-${arch}-unknown-none"
    myos_mlibc_link_shared "$arch" "$lib" -soname libdlsmoke.so "$ROOT/target/dl-dl_lib-mlibc-${arch}.o"
    myos_mlibc_link_shared "$arch" "$ROOT/target/dl-smoke-plugin-mlibc-${arch}-unknown-none" \
      "$ROOT/target/dl-dl_plugin-mlibc-${arch}.o"
    myos_mlibc_link_dynamic "$arch" "$ROOT/target/dl-smoke-mlibc-${arch}-unknown-none" \
      "$ROOT/target/dl-dl_smoke-mlibc-${arch}.o" "$lib" -rpath '$ORIGIN'
  fi
done

if [[ "$libc" == newlib ]]; then
  myos_c_smokes_version_hash > "$MYOS_C_SMOKES_VERSION"
else
  myos_c_smokes_mlibc_version_hash > "$MYOS_C_SMOKES_MLIBC_VERSION"
fi
echo "c smokes ($libc) -> target/{$(printf '%s\n' "${smokes[@]}" | cut -d' ' -f1 | paste -sd, -)}${suffix}-<arch>-unknown-none"
