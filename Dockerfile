# Multi-arch CI container for myos: pre-built system dependencies so the
# per-job `apt-get install` steps can be dropped from ci.yml / iso.yml.
#
# OS packages ONLY (QEMU, cross binutils, build tools, utilities). The Rust
# toolchain is intentionally NOT installed here: myos pins a specific nightly
# (rust-toolchain.toml), so each job installs it via dtolnay/rust-toolchain and
# stays reproducible against that exact toolchain.
#
# NOTE on the base image:
#   Base is ubuntu:26.04 (resolute). The riscv64 boot job needs RISC-V UEFI
#   firmware, and src/main.rs::riscv64_firmware() hard-requires either
#   /usr/share/qemu-efi-riscv64/RISCV_VIRT_CODE.fd or /usr/share/edk2/riscv64/
#   (no download fallback), which 22.04 lacks (the reverted #121). 26.04 over
#   24.04 for its QEMU (10.2): the AX201 boot test's device model is a
#   vfio-user server, and QEMU's vfio-user-pci client is 10.1 and later
#   (docs/wifi.md).
#
# NOTE on riscv64 binutils (differs from #121):
#   #121 shipped `binutils-riscv64-linux-gnu`. It exists on jammy+noble, but
#   nothing in this repo invokes any `riscv64-linux-gnu-*` binary: both riscv64
#   targets (kernel `riscv64imac-unknown-none-elf` and userspace
#   `riscv64-unknown-myos`) link with `rust-lld` (targets/*.json set
#   `linker = "rust-lld"`), and the C ports build riscv object files with clang.
#   The GNU riscv binutils are therefore dead weight and are dropped.
FROM --platform=$BUILDPLATFORM ubuntu:26.04

ARG TARGETARCH
ARG TARGETOS=linux

# GitHub Actions reusable environment
ENV CI=true \
    DEBIAN_FRONTEND=noninteractive \
    TZ=Etc/UTC

# Union of every system package apt-get-installed across ci.yml and iso.yml:
#   - qemu-system-{x86,arm,riscv} + qemu-efi-{aarch64,riscv64}: boot matrix
#     (bios/uefi/aarch64/riscv64) including the aarch64/riscv64 UEFI firmware
#     (riscv64 has its own package since QEMU 10, no longer qemu-system-misc).
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
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        qemu-system-x86 \
        qemu-system-arm \
        qemu-system-riscv \
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
    && rm -rf /var/lib/apt/lists/*

# Non-root CI user with passwordless sudo (available for interactive debugging).
# We intentionally do NOT `USER ciuser`: GitHub runs container jobs as root and
# sets HOME=/github/home, and the repo scripts rely on root-writable workspace
# + HOME semantics identical to a stock runner.
# uid/gid 1001: the stock ubuntu image already owns uid/gid 1000 with
# its built-in `ubuntu` user, and `useradd -u 1000 ciuser` errors out (exit 4)
# on that collision — so use a free id.
RUN useradd -m -u 1001 -s /bin/bash ciuser \
    && echo 'ciuser ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/ciuser

WORKDIR /workspace
ENTRYPOINT []
CMD ["/bin/bash", "-c", "sleep infinity"]