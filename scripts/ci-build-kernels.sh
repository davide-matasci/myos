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

# Build (idempotently: every script early-exits when its stamp is current)
# every port BEFORE the inputs hash is snapshotted below: the image ports'
# stamps are part of that hash, and a port missing from the registry (a
# skipped ports job, a new cache key) must not appear mid-build, which
# tripped the kernel_inputs_hash drift guard. The packages are built too:
# the boot jobs pack and serve them. The ports come in build order
# (scripts/ports.sh --build-list all: newlib and the sysroot first, then
# dependencies before dependents).
if [[ "${1:-}" != "--print-hash" && "${1:-}" != "--is-current" && "${1:-}" != "--print-members" ]]; then
  while read -r name script; do
    echo "==> ensure port $name ($script)"
    "./$script"
  done < <(./scripts/ports.sh --build-list all)
fi

STAMP="target/.myos-ci-kernel-version"

# Optional root-package features for the CI build (full boot sets
# MYOS_CI_FEATURES=linux_compat: the optional Linux layer loaded at boot and
# its musl test binaries in every image). The kernels, the `linux` module and
# the Linux layer's userspace are built either way; only the images differ.
CI_FEATURES="${MYOS_CI_FEATURES:-}"
FEATURE_ARGS=()
if [[ -n "$CI_FEATURES" ]]; then
  FEATURE_ARGS=(--features "$CI_FEATURES")
fi

# The Linux layer's musl files (linux-compat/build.sh, registry port
# `linux-compat`): in ci-build.tar always, so the ISO job and a full boot
# build their images from the same tar; the `linux` launcher is in
# HELLO_OK_ELFS (every image).
linux_compat_members() {
  local arch
  for arch in x86_64 aarch64 riscv64; do
    echo "target/linux-smoke-${arch}-linux-musl"
    echo "target/linux-compat/${arch}"
  done
}

# The version stamps of every image port, the toolchains included
# (scripts/ports.sh --stamps): they encode the userspace content
# kernel/build.rs embeds via include_bytes! and myos build.rs packs into the
# initramfs / Limine images.
PORT_STAMPS=()
while read -r stamp; do
  PORT_STAMPS+=("$stamp")
done < <(./scripts/ports.sh --stamps)

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
        # The port descriptors: which ports are in the image and what they ship.
        sha256sum scripts/ports.sh
        for f in ports/*/port.env user/*/port.env toolchain/*/port.env; do
          [[ -f "$f" ]] && sha256sum "$f"
        done
        # Whole host-bin crate (src/): target/debug/myos is the CI harness
        # (wait_ci) and the pack list ships it in ci-build.tar, so ANY src
        # change — not just the limine/initramfs files — must bust the stamp.
        # Existence-only artifacts_ready() made a rust-cache-restored binary
        # from an older commit pass freshness and boot stale harness needles.
        hash_tree src
        if [[ -f .cargo/config.toml ]]; then
          sha256sum .cargo/config.toml
        fi
        printf 'features:%s\n' "$CI_FEATURES"
        sha256sum linux-compat/launcher.c linux-compat/build-launcher.sh
        hash_tree linux-compat
        sha256sum linux-compat/build.sh
        # Port registry stamps encode userspace content identity (source-hash
        # philosophy). Do not hash target/ ELFs or manifests: those change or
        # appear after cargo clean/build and would make pull-tag ≠ push-tag.
        for stamp in "${PORT_STAMPS[@]+"${PORT_STAMPS[@]}"}"; do
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
        for f in "${CURL_MBEDTLS_INPUTS[@]+"${CURL_MBEDTLS_INPUTS[@]}"}"; do
          if [[ -f "$f" ]]; then
            sha256sum "$f"
          else
            printf 'input-missing:%s\n' "$f"
          fi
        done
        # tcc's libtcc1.a is installed into target/newlib-*/<triple>/lib by the
        # settle tcc build and baked into the x86 initramfs (initramfs
        # collect_tree => /lib/newlib/lib/libtcc1.a). It is not covered by the
        # newlib stamp, so hash it explicitly or a kernels artifact whose
        # initramfs lacks libtcc1.a would be reused forever (guest `tcc -o`
        # fails with 'libtcc1.a not found'). Produced before this hash snapshot
        # and never touched by cargo builds, so it cannot drift.
        for triple in x86_64-unknown-myos aarch64-unknown-myos riscv64-unknown-myos; do
          if [[ -f "target/libtcc1-${triple}.a" ]]; then
            printf 'libtcc1:%s:' "$triple"
            sha256sum "target/libtcc1-${triple}.a" | awk '{print $1}'
            printf '\n'
          else
            printf 'libtcc1-missing:%s\n' "$triple"
          fi
        done
      } | sha256sum | awk '{print $1}'
    )
  )"
  printf '%s' "$h"
}

# Diagnostics: one "<hash> <tag>" line per hashed contribution so a
# kernel_inputs_hash drift can be pinned to the exact input that changed.
# Purely additive — does NOT change kernel_inputs_hash, so the kernels registry
# pull/push tag stays byte-identical.
kernel_inputs_diag() {
  (
    cd "$ROOT"
    tree_diag() {
      local dir="$1"
      if [[ ! -d "$dir" ]]; then echo "MISSING-DIR $dir"; return 0; fi
      find "$dir" \
        \( -name target -o -path '*/target/*' \) -prune -o \
        -type f \( \
          -name '*.rs' -o -name '*.c' -o -name '*.h' -o -name '*.S' -o \
          -name 'Cargo.toml' -o -name 'build.rs' -o \
          -name 'link.ld' -o -name '*.ld' -o -name '*.json' -o -name '*.txt' \
        \) -print0 2>/dev/null \
        | sort -z | xargs -0 -r sha256sum
    }
    tree_diag kernel | sha256sum | awk -v d='tree:kernel' '{print $1" "d}'
    tree_diag modules | sha256sum | awk -v d='tree:modules' '{print $1" "d}'
    tree_diag user | sha256sum | awk -v d='tree:user' '{print $1" "d}'
    {
      sha256sum build.rs Cargo.toml 2>/dev/null || true
      sha256sum src/limine_image.rs src/limine_gpt.rs src/limine_fat.rs \
        src/limine_dir.rs src/initramfs.rs 2>/dev/null || true
    } | sha256sum | awk -v d='group:root-src' '{print $1" "d}'
    if [[ -f .cargo/config.toml ]]; then
      sha256sum .cargo/config.toml | awk -v d='file:.cargo/config.toml' '{print $1" "d}'
    fi
    for stamp in "${PORT_STAMPS[@]+"${PORT_STAMPS[@]}"}"; do
      if [[ -f "$stamp" ]]; then
        { printf 'stamp:%s:' "$stamp"; cat "$stamp"; printf '\n'; } \
          | sha256sum | awk -v s="$stamp" '{print $1" "s}'
      else
        printf 'MISSING %s\n' "$stamp"
      fi
    done
    for f in "${CURL_MBEDTLS_INPUTS[@]+"${CURL_MBEDTLS_INPUTS[@]}"}"; do
      if [[ -f "$f" ]]; then
        sha256sum "$f" | awk -v p="$f" '{print $1" "p}'
      else
        printf 'MISSING %s\n' "$f"
      fi
    done
    for triple in x86_64-unknown-myos aarch64-unknown-myos riscv64-unknown-myos; do
      if [[ -f "target/libtcc1-${triple}.a" ]]; then
        sha256sum "target/libtcc1-${triple}.a" \
          | awk -v t="$triple" '{print $1" libtcc1:"t}'
      else
        printf 'MISSING libtcc1:%s\n' "$triple"
      fi
    done
  ) | sort
}

# Host `myos aarch64/riscv64 --ci` rebuilds the guest disk image and reads these
# Limine modules from disk (not from the prebuilt kernel ELF). Every kernel
# module (src/limine_image.rs BOOT_MODULES) is in the list: the kernel embeds
# none, the images ship them under boot/modules/. GHCR kernels
# packages that omit them made master boot jobs panic with "hello ELF missing"
# after a kernels cache hit (PR builds were fine because they did a full cargo
# build). Keep them in artifacts_ready + --print-members. The ports' files
# (smokes, curl, dropbear, ...) are checked from the descriptors instead
# (scripts/ports.sh --all-files all in artifacts_ready).
HELLO_OK_ELFS=(
  target/console-x86_64-unknown-none
  target/console-aarch64-unknown-none-softfloat
  target/console-riscv64imac-unknown-none-elf
  target/stubfs-x86_64-unknown-none
  target/stubfs-aarch64-unknown-none-softfloat
  target/stubfs-riscv64imac-unknown-none-elf
  target/hello-x86_64-unknown-none
  target/hello-aarch64-unknown-none-softfloat
  target/hello-riscv64imac-unknown-none-elf
  target/pci_enum-x86_64-unknown-none
  target/pci_enum-aarch64-unknown-none-softfloat
  target/pci_enum-riscv64imac-unknown-none-elf
  target/acpi-x86_64-unknown-none
  target/acpi-aarch64-unknown-none-softfloat
  target/acpi-riscv64imac-unknown-none-elf
  target/virtio_blk-x86_64-unknown-none
  target/virtio_blk-aarch64-unknown-none-softfloat
  target/virtio_blk-riscv64imac-unknown-none-elf
  target/nvme-x86_64-unknown-none
  target/nvme-aarch64-unknown-none-softfloat
  target/nvme-riscv64imac-unknown-none-elf
  target/virtio_net-x86_64-unknown-none
  target/virtio_net-aarch64-unknown-none-softfloat
  target/virtio_net-riscv64imac-unknown-none-elf
  target/netfs-x86_64-unknown-none
  target/netfs-aarch64-unknown-none-softfloat
  target/netfs-riscv64imac-unknown-none-elf
  target/fat-x86_64-unknown-none
  target/fat-aarch64-unknown-none-softfloat
  target/fat-riscv64imac-unknown-none-elf
  target/ext2-x86_64-unknown-none
  target/ext2-aarch64-unknown-none-softfloat
  target/ext2-riscv64imac-unknown-none-elf
  target/linux-x86_64-unknown-none
  target/linux-aarch64-unknown-none-softfloat
  target/linux-riscv64imac-unknown-none-elf
  target/linux-launcher-x86_64-unknown-none
  target/linux-launcher-aarch64-unknown-none
  target/linux-launcher-riscv64-unknown-none
  target/ok-x86_64-unknown-none
  target/ok-aarch64-unknown-none-softfloat
  target/ok-riscv64imac-unknown-none-elf
)

# Everything the images are packed from: the kernels, the modules and every
# file the ports ship (scripts/ports.sh --all-files all: a boot job re-packs
# the aarch64/riscv64 initramfs from the image ports' and the packages from
# all of them), plus the Linux layer.
artifacts_ready() {
  [[ -x target/debug/myos ]] \
    && [[ -f target/bios.img ]] \
    && [[ -f target/uefi.img ]] \
    && [[ -f target/aarch64-unknown-none-softfloat/debug/kernel ]] \
    && [[ -f target/riscv64imac-unknown-none-elf/debug/kernel ]] \
    || return 1
  local f
  for f in "${HELLO_OK_ELFS[@]+"${HELLO_OK_ELFS[@]}"}"; do
    [[ -f "$f" ]] || return 1
  done
  while read -r f; do
    [[ -e "$f" ]] || return 1
  done < <(./scripts/ports.sh --all-files all)
  for f in $(linux_compat_members); do
    [[ -e "$f" ]] || return 1
  done
  for f in x86_64 aarch64 riscv64; do
    [[ -f "target/linux-compat/$f/get-alpine" ]] || return 1
  done
  return 0
}

do_clean_and_build() {
  echo "==> kernel inputs changed or artifacts missing; clean + build"
  # The ports were built above (before the hash snapshot); build.rs only
  # checks them. Clean so the images and the embeds are rebuilt from them.
  cargo clean -p myos
  # Artifact-dep kernel skips build.rs when ELFs change but sources do not;
  # stale include_bytes! in bootfs caused x86 #GP after std cat ok in CI.
  cargo clean -p kernel --target x86_64-unknown-none
  cargo clean -p kernel --target aarch64-unknown-none-softfloat
  cargo clean -p kernel --target riscv64imac-unknown-none-elf
  # The `linux` launcher is in every initramfs (the layer is a module); the
  # musl pieces come from the registry (`linux-compat`) or are built here.
  "$ROOT/linux-compat/build-launcher.sh"
  "$ROOT/linux-compat/build.sh"
  cargo build "${FEATURE_ARGS[@]+"${FEATURE_ARGS[@]}"}"
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
    for f in "${HELLO_OK_ELFS[@]+"${HELLO_OK_ELFS[@]}"}"; do
      echo "$f"
    done
    linux_compat_members
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
diag_before="$(kernel_inputs_diag)"

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
  echo "error: changed input members (hash <tag>):" >&2
  diff <(printf '%s\n' "$diag_before") <(printf '%s\n' "$(kernel_inputs_diag)") >&2 || true
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
