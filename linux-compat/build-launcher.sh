#!/usr/bin/env bash
# Build the `linux` launcher (linux-compat/launcher.c, a native myos program
# on newlib) for x86_64, aarch64 and riscv64:
#   target/linux-launcher-<arch>-unknown-none
# It ships in every image (`/bin/etc/linux`), since the Linux layer itself is
# a kernel module (`/lib/modules/linux`, loaded at boot with `--features
# linux_compat` or by hand with `insmod`). The musl pieces (the tests and
# get-alpine) are linux-compat/build.sh. Skips arches whose launcher is newer
# than its inputs.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

ARCHES=(x86_64 aarch64 riscv64)
SRC="$ROOT/linux-compat/launcher.c"

myos_ensure_llvm_bin
"$ROOT/toolchain/newlib/build.sh"
"$ROOT/toolchain/newlib/tool-wrappers.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

for arch in "${ARCHES[@]}"; do
  out="$ROOT/target/linux-launcher-$arch-unknown-none"
  triple="$arch-unknown-myos"
  nl="$ROOT/target/newlib-$arch"
  # A libgloss change (the syscalls it makes) needs a new launcher too.
  if [[ -f "$out" && "$out" -nt "$SRC" && "$out" -nt "${BASH_SOURCE[0]}" \
        && "$out" -nt "$nl/$triple/lib/libgloss.a" ]]; then
    continue
  fi
  echo "==> linux launcher ($arch, myos newlib)"
  obj="$ROOT/target/linux-launcher-$arch.o"
  "${triple}-cc" -ffreestanding -fPIC -O2 -isystem "$nl/$triple/include" \
    -c "$SRC" -o "$obj"
  ld.lld -pie --no-dynamic-linker -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$nl/$triple/lib/crt0.o" "$obj" -L"$nl/$triple/lib" \
    --start-group -lc -lgloss -lg --end-group
done
echo "linux launcher -> target/linux-launcher-{x86_64,aarch64,riscv64}-unknown-none"
