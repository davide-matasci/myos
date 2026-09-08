# Multi-arch CI runner with pre-installed system tools and QEMU for all targets
# Used by this repo's GitHub Actions CI to eliminate per-job apt-get install
FROM --platform=$BUILDPLATFORM ubuntu:22.04

# Preserve build args for multi-arch consistency
ARG TARGETARCH
ARG TARGETOS=linux

# GitHub Actions reusable environment
ENV CI=true \
    DEBIAN_FRONTEND=noninteractive \
    TZ=Etc/UTC

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        # QEMU / emulators for all CI targets (bios/uefi/aarch64/riscv64)
        qemu-system-x86 \
        qemu-system-arm \
        qemu-system-misc \
        qemu-efi-aarch64 \
        # Build toolchain + cross toolchains used by kernel/port builds
        clang lld \
        binutils-aarch64-linux-gnu \
        binutils-riscv64-linux-gnu \
        gcc g++ make \
        autoconf automake libtool \
        texinfo bison flex \
        pkg-config \
        zstd rsync \
        # Common developer utilities
        git curl wget file patch bc \
        sudo \
    && rm -rf /var/lib/apt/lists/*

# Add a non-privileged user for CI steps
RUN useradd -m -u 1000 -s /bin/bash ciuser \
    && echo 'ciuser ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/ciuser

WORKDIR /workspace

# Optional: cache git objects (saves network in repeated runs)
RUN git config --global advice.detachedHead false

# Create a symlink to typical local user home for scripts
RUN mkdir -p /home/ciuser && chown ciuser:ciuser /home/ciuser

# Default command (easy for debugging)
CMD ["/bin/bash", "-c", "while true; do sleep 3600; done"]
