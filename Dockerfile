# Multi-arch CI container for myos: pre-built system dependencies so the
# per-job `apt-get install` steps can be dropped from ci.yml / iso.yml.
#
# OS packages ONLY (QEMU, cross binutils, build tools, utilities). The Rust
# toolchain is intentionally NOT installed here: myos pins a specific nightly
# (rust-toolchain.toml), so each job installs it via dtolnay/rust-toolchain and
# stays reproducible against that exact toolchain.
#
# NOTE on the base image vs the reverted #121:
#   Base is ubuntu:24.04 (noble), NOT 22.04. The riscv64 boot job needs RISC-V
#   UEFI firmware, and src/main.rs::riscv64_firmware() hard-requires either
#   /usr/share/qemu-efi-riscv64/RISCV_VIRT_CODE.fd or /usr/share/edk2/riscv64/
#   (no download fallback). qemu-efi-riscv64 does not exist on 22.04, so a
#   jammy container cannot pass the riscv64 boot. 24.04 is also what the
#   ubuntu-latest runner currently ships, so clang/lld/qemu versions in the
#   image match the versions the jobs were verified against on the runner.
#
# NOTE on riscv64 binutils (differs from #121):
#   #121 shipped `binutils-riscv64-linux-gnu`. It exists on jammy+noble, but
#   nothing in this repo invokes any `riscv64-linux-gnu-*` binary: both riscv64
#   targets (kernel `riscv64imac-unknown-none-elf` and userspace
#   `riscv64-unknown-myos`) link with `rust-lld` (targets/*.json set
#   `linker = "rust-lld"`), and the C ports build riscv object files with clang.
#   The GNU riscv binutils are therefore dead weight and are dropped.
# QEMU 10.1 for riscv64: the RISC-V IOMMU (`-device riscv-iommu-pci`,
# `kernel/src/iommu/riscv.rs`, `docs/pci-acpi-smp.md`) is QEMU 10.1's; noble's
# 8.2 has none, so every riscv64 boot would run without one. Built from the
# release tarball here (one target, slirp for the user network the boots
# use, the system libfdt) and copied into the image below, ahead of
# qemu-system-misc's binary on the PATH through /usr/local/bin.
FROM --platform=$BUILDPLATFORM ubuntu:24.04 AS qemu-riscv64
ARG QEMU_VERSION=10.1.0
ARG QEMU_SHA256=e0517349b50ca73ebec2fa85b06050d5c463ca65c738833bd8fc1f15f180be51
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        build-essential ninja-build python3 python3-venv pkg-config \
        libglib2.0-dev libpixman-1-dev libslirp-dev libfdt-dev zlib1g-dev \
        curl ca-certificates xz-utils \
    && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL -o /tmp/qemu.tar.xz "https://download.qemu.org/qemu-${QEMU_VERSION}.tar.xz" \
    && echo "${QEMU_SHA256}  /tmp/qemu.tar.xz" | sha256sum -c - \
    && mkdir /tmp/qemu && tar -xJf /tmp/qemu.tar.xz -C /tmp/qemu --strip-components=1 \
    && cd /tmp/qemu \
    && ./configure --target-list=riscv64-softmmu --prefix=/opt/qemu-riscv64 \
        --enable-slirp --enable-fdt=system \
        --disable-docs --disable-gtk --disable-sdl --disable-vnc --disable-werror \
    && make -j"$(nproc)" && make install \
    && rm -rf /tmp/qemu /tmp/qemu.tar.xz

FROM --platform=$BUILDPLATFORM ubuntu:24.04

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
#   - openssh-client: full-boot dropbear SSH smoke (host → guest via hostfwd).
#   - xz-utils: .tar.xz upstream tarballs (fontconfig); the fetch scripts fall
#     back to python3's lzma on an image without it.
#   - e2fsprogs: the ext2 host tests and the boots' scratch-disk e2fsck.
#   - dosfstools, mtools: the FAT host tests (fatvol) and the boots'
#     fsck.fat of the FAT test disk.
#   - libslirp0, libfdt1 (libglib2.0-0, libpixman-1-0 and zlib come with
#     qemu-system-*): what the QEMU 10.1 riscv64 binary below links.
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
        openssh-client \
        e2fsprogs \
        dosfstools mtools \
        sudo \
        ca-certificates \
        python3 \
        xz-utils \
        libslirp0 libfdt1 \
    && rm -rf /var/lib/apt/lists/*

# QEMU 10.1's qemu-system-riscv64 (the stage above) ahead of 8.2's on the
# PATH: the riscv64 boots get the RISC-V IOMMU.
COPY --from=qemu-riscv64 /opt/qemu-riscv64 /opt/qemu-riscv64
RUN ln -s /opt/qemu-riscv64/bin/qemu-system-riscv64 /usr/local/bin/qemu-system-riscv64

# Non-root CI user with passwordless sudo (available for interactive debugging).
# We intentionally do NOT `USER ciuser`: GitHub runs container jobs as root and
# sets HOME=/github/home, and the repo scripts rely on root-writable workspace
# + HOME semantics identical to a stock runner.
# uid/gid 1001: the stock ubuntu:24.04 image already owns uid/gid 1000 with
# its built-in `ubuntu` user, and `useradd -u 1000 ciuser` errors out (exit 4)
# on that collision — so use a free id.
RUN useradd -m -u 1001 -s /bin/bash ciuser \
    && echo 'ciuser ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/ciuser

WORKDIR /workspace
ENTRYPOINT []
CMD ["/bin/bash", "-c", "sleep infinity"]