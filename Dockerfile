# Multi-arch CI container for myos: pre-built system dependencies so the
# per-job `apt-get install` steps can be dropped from ci.yml / iso.yml.
#
# OS packages ONLY (QEMU, cross binutils, build tools, utilities). The Rust
# toolchain is intentionally NOT installed here: myos pins a specific nightly
# (rust-toolchain.toml), so each job installs it via dtolnay/rust-toolchain and
# stays reproducible against that exact toolchain.
#
# NOTE on riscv64 binutils (differs from the reverted #121):
#   #121 shipped `binutils-riscv64-linux-gnu`. The package DOES exist on jammy
#   (2.38), but nothing in this repo invokes any `riscv64-linux-gnu-*` binary:
#   both riscv64 targets (kernel `riscv64imac-unknown-none-elf` and userspace
#   `riscv64-unknown-myos`) link with `rust-lld` (targets/*.json set
#   `linker = "rust-lld"`), and the C ports build riscv object files with clang.
#   The GNU riscv binutils are therefore dead weight (~9 MB) and are dropped.
FROM --platform=$BUILDPLATFORM ubuntu:22.04

ARG TARGETARCH
ARG TARGETOS=linux

# GitHub Actions reusable environment
ENV CI=true \
    DEBIAN_FRONTEND=noninteractive \
    TZ=Etc/UTC

# Union of every system package apt-get-installed across ci.yml and iso.yml:
#   - qemu-system-{x86,arm,misc} + qemu-efi-{aarch64,riscv64}: boot matrix
#     (bios/uefi/aarch64/riscv64) including the aarch64/riscv64 UEFI firmware.
#   - clang lld binutils-aarch64-linux-gnu: cross link for aarch64 userspace
#     ELFs (newlib/ports) + kernel/user EE linking.
#   - gcc g++ make autoconf automake libtool texinfo bison flex pkg-config:
#     host builds + configure-based GNU ports (newlib, git, vim, lynx, tcc...).
#   - zstd rsync xorriso: sysroot packaging + fetch, iso.yml ISO image.
#   - git curl wget file patch bc ca-certificates python3: source fetch, SSL,
#     checkout sync, wire-myos.py / port patching.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        qemu-system-x86 \
        qemu-system-arm \
        qemu-system-misc \
        qemu-efi-aarch64 \
        qemu-efi-riscv64 \
        clang lld \
        binutils-aarch64-linux-gnu \
        gcc g++ make \
        autoconf automake libtool \
        texinfo bison flex \
        pkg-config \
        zstd rsync \
        xorriso \
        git curl wget file patch bc \
        sudo \
        ca-certificates \
        python3 \
    && rm -rf /var/lib/apt/lists/*

# Non-root CI user with passwordless sudo (available for interactive debugging).
# We intentionally do NOT `USER ciuser`: GitHub runs container jobs as root and
# sets HOME=/github/home, and the repo scripts rely on root-writable workspace
# + HOME semantics identical to a stock runner.
RUN useradd -m -u 1000 -s /bin/bash ciuser \
    && echo 'ciuser ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/ciuser

WORKDIR /workspace
ENTRYPOINT []
CMD ["/bin/bash", "-c", "sleep infinity"]