#!/usr/bin/env bash
# The C smoke programs of the boot tests (user/c, the `c-smokes` port):
# c-hello and socket_smoke (build-c-hello.sh, the kernel embeds c-hello), and
# the ones below, each a static PIE against newlib + libgloss/myos for the
# three arches: target/<name>-<arch>-unknown-none.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_c_smokes_is_current; then
  echo "c smokes up to date"
  exit 0
fi

"$ROOT/scripts/build-c-hello.sh"
"$ROOT/toolchain/newlib/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"
myos_ensure_llvm_bin

# name -> source
smokes=(
  "tcp-listen-smoke user/c/tcp_listen_smoke.c"
  "pty-smoke user/c/pty_smoke.c"
  "urandom-smoke user/c/urandom_smoke.c"
  "tty-smoke user/c/tty_smoke.c"
  "unix-smoke user/c/unix_smoke.c"
  "fb-smoke user/c/fb_smoke.c"
  "poll-smoke user/c/poll_smoke.c"
  "kbd-smoke user/c/kbd_smoke.c"
  "uio-smoke user/c/uio_smoke.c"
  "pthread-smoke user/c/pthread_smoke.c"
  "netconv-smoke user/c/netconv_smoke.c"
  "child-smoke user/c/child_smoke.c"
  "libc-smoke user/c/libc_smoke.c"
  "sec user/c/sec.c"
  "at-smoke user/c/at_smoke.c"
  "fileio-smoke user/c/fileio_smoke.c"
  "crash-smoke user/c/crash_smoke.c"
)
for arch in x86_64 aarch64 riscv64; do
  triple="${arch}-unknown-myos"
  prefix="$ROOT/target/newlib-${arch}"
  inc="$prefix/${triple}/include"
  lib="$prefix/${triple}/lib"
  cc="${triple}-cc"
  # newlib's printf wants the long-double and soft-float helpers these
  # arches lack in the shim (the sbase port carries them).
  extra=()
  if [[ "$arch" == "aarch64" ]]; then
    tf="$ROOT/target/c-smokes-${arch}-trunctfdf2.o"
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" -c "$ROOT/ports/sbase/trunctfdf2.c" -o "$tf"
    extra+=("$tf")
  elif [[ "$arch" == "riscv64" ]]; then
    sf="$ROOT/target/c-smokes-${arch}-softfloat.o"
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" -c "$ROOT/ports/sbase/riscv64-softfloat.c" -o "$sf"
    extra+=("$sf" "$(myos_riscv64_softfloat)")
  fi
  for entry in "${smokes[@]}"; do
    name="${entry%% *}"
    src="${entry#* }"
    out="$ROOT/target/${name}-${arch}-unknown-none"
    obj="$ROOT/target/${name}-${arch}.o"
    echo "==> ${name} ($triple)"
    "$cc" -ffreestanding -fPIC -O2 -isystem "$inc" -c "$ROOT/$src" -o "$obj"
    ld.lld -pie --no-dynamic-linker -o "$out" \
      --entry=_start -z max-page-size=4096 \
      "$lib/crt0.o" "$obj" "${extra[@]+"${extra[@]}"}" -L"$lib" \
      --start-group -lc -lgloss -lg --end-group
  done
done

myos_c_smokes_version_hash > "$MYOS_C_SMOKES_VERSION"
echo "c smokes -> target/{c-hello,c-socket_smoke,tcp-listen-smoke,pty-smoke,urandom-smoke,tty-smoke,unix-smoke,fb-smoke,poll-smoke,kbd-smoke,uio-smoke,pthread-smoke,netconv-smoke,child-smoke,libc-smoke,sec,at-smoke,fileio-smoke}-<arch>-unknown-none"
