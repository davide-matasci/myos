#!/usr/bin/env bash
# Emit cross-tool wrappers for newlib (clang + ld.lld on every arch).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
BIN="$ROOT/target/newlib-bin"
mkdir -p "$BIN"

write_wrapper() {
  local name="$1"
  local body="$2"
  printf '%s\n' "$body" > "$BIN/$name"
  chmod +x "$BIN/$name"
}

for arch in x86_64 aarch64 riscv64; do
  triple="${arch}-unknown-myos"
  elf="${arch}-unknown-none"
  ld='ld.lld'
  write_wrapper "${triple}-cc" "#!/usr/bin/env bash
exec clang --target=${elf} -ffreestanding -fPIC \"\$@\"
"
  write_wrapper "${triple}-c++" "#!/usr/bin/env bash
exec clang++ --target=${elf} -ffreestanding -fPIC \"\$@\"
"
  write_wrapper "${triple}-as" "#!/usr/bin/env bash
exec clang --target=${elf} -c \"\$@\"
"
  write_wrapper "${triple}-ld" "#!/usr/bin/env bash
exec ${ld} \"\$@\"
"
  write_wrapper "${triple}-nm" "#!/usr/bin/env bash
exec nm \"\$@\"
"
  write_wrapper "${triple}-objcopy" "#!/usr/bin/env bash
exec objcopy \"\$@\"
"
  write_wrapper "${triple}-objdump" "#!/usr/bin/env bash
exec objdump \"\$@\"
"
  write_wrapper "${triple}-ranlib" "#!/usr/bin/env bash
exec ranlib \"\$@\"
"
  write_wrapper "${triple}-strip" "#!/usr/bin/env bash
exec strip \"\$@\"
"
  write_wrapper "${triple}-ar" "#!/usr/bin/env bash
exec ar \"\$@\"
"
done

echo "newlib cross tools -> $BIN"
