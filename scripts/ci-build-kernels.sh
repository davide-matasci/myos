#!/usr/bin/env bash
# Hash-gated kernel + Limine BIOS/UEFI image build for CI.
#
# Same content-hash philosophy as ports/sysroot: if inputs are unchanged and
# required artifacts exist, exit 0 without cargo clean/build (CI re-run no-op).
# If inputs changed (kernel/host sources, or userspace ELF bytes that
# include_bytes! / initramfs bake in), clean + rebuild like before.
#
# Artifact-dep kernels skip build.rs when ELF paths are stable but bytes
# change; unconditional clean used to paper over that. The stamp + digests
# below replace that every-run clean.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

STAMP="target/.myos-ci-kernel-version"

# Stable ELF copies that kernel/build.rs embed via include_bytes! / rustc-env,
# and that myos build.rs packs into initramfs / Limine images.
PORT_STAMPS=(
  target/.myos-std-hello-version
  target/.myos-c-hello-version
  target/.myos-oksh-version
  target/.myos-ubase-version
  target/.myos-sbase-version
  target/.myos-coreutils-version
  target/.myos-ripgrep-version
  target/.myos-tcc-version
  target/.myos-newlib-version
)

hash_tree() {
  # Checkout-stable: relative paths, sorted, no abs paths, no target/ junk.
  local dir="$1"
  if [[ ! -d "$dir" ]]; then
    return 0
  fi
  find "$dir" \
    \( -name target -o -path '*/target/*' \) -prune -o \
    -type f \( \
      -name '*.rs' -o -name '*.c' -o -name '*.h' -o -name '*.S' -o \
      -name 'Cargo.toml' -o -name 'Cargo.lock' -o -name 'build.rs' -o \
      -name 'link.ld' -o -name '*.ld' -o -name '*.json' -o -name '*.txt' \
    \) -print0 2>/dev/null \
    | sort -z | xargs -0 -r sha256sum
}

kernel_inputs_hash() {
  local h
  h="$(
    (
      cd "$ROOT"
      {
        # Kernel + nested crates that kernel/build.rs compiles into embeds.
        hash_tree kernel
        hash_tree modules
        hash_tree user
        # Host myos bits that bake kernel/initramfs into bios/uefi images.
        sha256sum build.rs Cargo.toml Cargo.lock 2>/dev/null || true
        sha256sum \
          src/limine_image.rs \
          src/limine_gpt.rs \
          src/limine_fat.rs \
          src/limine_dir.rs \
          src/initramfs.rs \
          2>/dev/null || true
        if [[ -f .cargo/config.toml ]]; then
          sha256sum .cargo/config.toml
        fi
        # Port registry stamps (source-hash philosophy) + ELF digests so
        # include_bytes! / initramfs freshness is not lost when bytes move
        # without a cargo fingerprint bump.
        for stamp in "${PORT_STAMPS[@]}"; do
          if [[ -f "$stamp" ]]; then
            # Label + contents: relative path in the stream.
            printf 'stamp:%s:' "$stamp"
            cat "$stamp"
            printf '\n'
          else
            printf 'stamp-missing:%s\n' "$stamp"
          fi
        done
        shopt -s nullglob
        # Relative-path digests only (never hash manifest files: they embed
        # absolute $ROOT paths and would make the stamp machine-dependent).
        for f in \
          target/std-hello-*-unknown-myos \
          target/std-cat-*-unknown-myos \
          target/std-echo-*-unknown-myos \
          target/std-bigalloc-*-unknown-myos \
          target/c-hello-*-unknown-none \
          target/oksh-*-unknown-none \
          target/coreutils-*-unknown-myos \
          target/rg-*-unknown-myos \
          target/tcc-*-unknown-myos
        do
          [[ -f "$f" ]] || continue
          sha256sum "$f"
        done
        # Manifests may list abs paths; record name + content digest only.
        for f in target/ubase-manifest-*.txt target/sbase-manifest-*.txt \
                 target/coreutils-manifest-*.txt; do
          [[ -f "$f" ]] || continue
          # Label by basename (arch) so abs path to the manifest is not hashed.
          printf 'manifest:%s\n' "$(basename "$f")"
          while IFS= read -r line; do
            line="${line#"${line%%[![:space:]]*}"}"
            [[ -z "$line" || "$line" == \#* ]] && continue
            if [[ "$line" == *:* ]]; then
              name="${line%%:*}"
              path="${line#*:}"
            else
              name="$line"
              path=""
            fi
            if [[ -n "$path" && -f "$path" ]]; then
              printf '%s:%s\n' "$name" "$(sha256sum "$path" | awk '{print $1}')"
            elif [[ -z "$path" ]]; then
              # coreutils-manifest is names-only; ELF hashed via glob above.
              printf 'name:%s\n' "$name"
            else
              printf '%s:missing\n' "$name"
            fi
          done <"$f"
        done
        shopt -u nullglob
      } | sha256sum | awk '{print $1}'
    )
  )"
  printf '%s' "$h"
}

artifacts_ready() {
  [[ -x target/debug/myos ]] \
    && [[ -f target/bios.img ]] \
    && [[ -f target/uefi.img ]] \
    && [[ -f target/aarch64-unknown-none-softfloat/debug/kernel ]] \
    && [[ -f target/riscv64imac-unknown-none-elf/debug/kernel ]]
}

do_clean_and_build() {
  echo "==> kernel inputs changed or artifacts missing; clean + build"
  cargo clean -p myos
  # Artifact-dep kernel skips build.rs when ELFs change but sources do not;
  # stale include_bytes! in bootfs caused x86 #GP after std cat ok in CI.
  cargo clean -p kernel --target x86_64-unknown-none
  cargo clean -p kernel --target aarch64-unknown-none-softfloat
  cargo clean -p kernel --target riscv64imac-unknown-none-elf
  cargo build
  cargo build -p kernel --target aarch64-unknown-none-softfloat
  cargo build -p kernel --target riscv64imac-unknown-none-elf
}

mkdir -p target

# Subcommands for scripts/ci-registry.sh kernels port.
case "${1:-}" in
  --print-hash)
    kernel_inputs_hash
    printf '\n'
    exit 0
    ;;
  --print-members)
    echo target/.myos-ci-kernel-version
    echo target/debug/myos
    echo target/bios.img
    echo target/uefi.img
    echo target/fat.img
    echo target/aarch64-unknown-none-softfloat/debug/kernel
    echo target/riscv64imac-unknown-none-elf/debug/kernel
    exit 0
    ;;
  --is-current)
    want="$(kernel_inputs_hash)"
    if [[ -f "$STAMP" ]] && [[ "$(cat "$STAMP")" == "$want" ]] && artifacts_ready; then
      exit 0
    fi
    exit 1
    ;;
esac

want="$(kernel_inputs_hash)"

# GHCR pull (same content-hash philosophy as ports). Ignore pull failures.
if [[ -x "$ROOT/scripts/ci-registry.sh" ]]; then
  "$ROOT/scripts/ci-registry.sh" pull kernels || true
fi

have=""
if [[ -f "$STAMP" ]]; then
  have="$(cat "$STAMP")"
fi

if [[ -n "$have" && "$have" == "$want" ]] && artifacts_ready; then
  echo "kernels up to date ($want)"
  ls -lh target/debug/myos target/bios.img target/uefi.img \
    target/aarch64-unknown-none-softfloat/debug/kernel \
    target/riscv64imac-unknown-none-elf/debug/kernel
  exit 0
fi

if [[ -n "$have" && "$have" != "$want" ]]; then
  echo "==> kernel input hash mismatch (was ${have:0:12}…, now ${want:0:12}…)"
elif [[ -z "$have" ]]; then
  echo "==> no kernel version stamp; building"
else
  echo "==> kernel artifacts incomplete; building"
fi

do_clean_and_build

# Recompute after build in case digests of newly-written nested stable copies
# are not part of the stamp (nested ELFs are outputs). Stamp is inputs-only.
want="$(kernel_inputs_hash)"
printf '%s\n' "$want" >"$STAMP"

if ! artifacts_ready; then
  echo "error: kernel build finished but required artifacts are missing" >&2
  ls -la target/debug/myos target/bios.img target/uefi.img \
    target/aarch64-unknown-none-softfloat/debug/kernel \
    target/riscv64imac-unknown-none-elf/debug/kernel >&2 || true
  exit 1
fi

echo "kernels built; stamp $want"
test -f target/bios.img
od -An -tx1 -N 16 target/bios.img

# Persist stamp+artifacts to GHCR so the next CI re-run is a true no-op
# (rust-cache does not re-save on an exact key hit).
if [[ -x "$ROOT/scripts/ci-registry.sh" ]]; then
  "$ROOT/scripts/ci-registry.sh" push kernels || true
fi
