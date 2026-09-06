#!/usr/bin/env bash
# Hash-gated kernel + Limine BIOS/UEFI image build for CI.
#
# Same content-hash philosophy as ports/sysroot: if inputs are unchanged and
# required artifacts exist, exit 0 without cargo clean/build (CI re-run no-op).
# If inputs changed (kernel/host sources, or port registry stamps that encode
# userspace content identity for include_bytes! / initramfs), clean + rebuild.
#
# Hash must be stable across a clean→build cycle: do NOT digest target/ ELFs or
# manifests (those change or appear after cargo build). Port stamps already
# capture userspace identity; hashing ELF bytes caused pull-tag ≠ push-tag in
# the same CI job (GHCR miss on re-run → full kernel recompile).
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
  target/.myos-vim-version
  target/.myos-ncurses-version
  target/.myos-newlib-version
)

# Source-controlled curl/mbedtls inputs (NOT target/.myos-{curl,mbedtls}-version).
# Those stamps are rewritten by ports/*/build.sh during this same job (and mbedtls
# WANT also shifts when target/cacert.pem appears mid-fetch), which made
# kernel_inputs_hash drift after build. Hash pins + config/patches instead.
CURL_MBEDTLS_INPUTS=(
  ports/mbedtls/versions.env
  ports/mbedtls/myos_mbedtls_config.h
  ports/mbedtls/build.sh
  ports/mbedtls/fetch.sh
  ports/curl/versions.env
  ports/curl/config-myos.h
  ports/curl/build.sh
  ports/curl/fetch.sh
  ports/curl/build-softfloat-riscv64.sh
  ports/curl/myos_curl_platform.c
  ports/curl/mbedtls.c.myos.patch
  ports/curl/tool_cfgable.h.myos.patch
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
      -name 'Cargo.toml' -o -name 'build.rs' -o \
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
        # Do not hash Cargo.lock: **/Cargo.lock is gitignored and appears after
        # the first cargo build, which made pull-tag ≠ post-build stamp.
        sha256sum build.rs Cargo.toml 2>/dev/null || true
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
        # Port registry stamps encode userspace content identity (source-hash
        # philosophy). Do not hash target/ ELFs or manifests: those change or
        # appear after cargo clean/build and would make pull-tag ≠ push-tag.
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
        # curl/mbedtls: digest checkout-stable sources only (see CURL_MBEDTLS_INPUTS).
        for f in "${CURL_MBEDTLS_INPUTS[@]}"; do
          if [[ -f "$f" ]]; then
            sha256sum "$f"
          else
            printf 'input-missing:%s\n' "$f"
          fi
        done
      } | sha256sum | awk '{print $1}'
    )
  )"
  printf '%s' "$h"
}

# Host `myos aarch64/riscv64 --ci` rebuilds the guest disk image and reads these
# Limine modules from disk (not from the prebuilt kernel ELF). GHCR kernels
# packages that omit them made master boot jobs panic with "hello ELF missing"
# after a kernels cache hit (PR builds were fine because they did a full cargo
# build). Keep them in artifacts_ready + --print-members.
#
# socket_smoke + curl: same class of bug for #109 — build job must produce and
# pack them or aarch64/riscv initramfs skips and `[ OK ] socket` / interactive
# curl fail. Canonical names plus coreutils-* pack aliases (ci.yml glob).
HELLO_OK_ELFS=(
  target/hello-x86_64-unknown-none
  target/hello-aarch64-unknown-none-softfloat
  target/hello-riscv64imac-unknown-none-elf
  target/ok-x86_64-unknown-none
  target/ok-aarch64-unknown-none-softfloat
  target/ok-riscv64imac-unknown-none-elf
  target/c-socket_smoke-x86_64-unknown-none
  target/c-socket_smoke-aarch64-unknown-none
  target/c-socket_smoke-riscv64-unknown-none
  target/curl-x86_64-unknown-none
  target/curl-aarch64-unknown-none
  target/curl-riscv64-unknown-none
  target/coreutils-c-socket_smoke-x86_64-unknown-none
  target/coreutils-c-socket_smoke-aarch64-unknown-none
  target/coreutils-c-socket_smoke-riscv64-unknown-none
  target/coreutils-curl-x86_64-unknown-none
  target/coreutils-curl-aarch64-unknown-none
  target/coreutils-curl-riscv64-unknown-none
)

artifacts_ready() {
  [[ -x target/debug/myos ]] \
    && [[ -f target/bios.img ]] \
    && [[ -f target/uefi.img ]] \
    && [[ -f target/aarch64-unknown-none-softfloat/debug/kernel ]] \
    && [[ -f target/riscv64imac-unknown-none-elf/debug/kernel ]] \
    || return 1
  local f
  for f in "${HELLO_OK_ELFS[@]}"; do
    [[ -f "$f" ]] || return 1
  done
  return 0
}

do_clean_and_build() {
  echo "==> kernel inputs changed or artifacts missing; clean + build"
  # Ensure socket_smoke + curl exist before initramfs/cargo (x86 bios embeds them).
  "$ROOT/scripts/build-c-hello.sh"
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
    for f in "${HELLO_OK_ELFS[@]}"; do
      echo "$f"
    done
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

# Stamp the pre-build want: must match the GHCR pull/push tag from --print-hash
# (ci-registry.sh). Never recompute after build — ELF appearance under target/
# must not change the hash.
after="$(kernel_inputs_hash)"
if [[ "$after" != "$want" ]]; then
  echo "error: kernel_inputs_hash drifted after build (before ${want:0:12}…, after ${after:0:12}…)" >&2
  echo "error: hashing must be inputs-only (sources + port stamps); refusing to stamp" >&2
  exit 1
fi
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
