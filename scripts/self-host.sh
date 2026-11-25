#!/bin/sh
# self-host.sh [-r APPS] [CARGO ARGS...]
#
# Build this tree's kernel on myos with myos's own compiler: the rustc
# package's cargo and rustc, and the llvm package's lld, all native
# programs. For the arch it runs on, from the crates in vendor/, with no
# network and no Linux layer (docs/userspace.md "Building myos on myos").
#
# The tree is the myos-src package's (packages/myos-src: the source the
# system was built from, with its crates), installed on a disk: the build
# writes a few GiB under target/.
#
#     mkfs.ext2 /dev/nvme1n1/data && mount /dev/nvme1n1/data /mnt ext2
#     get-myos -r /mnt/apps myos-src        # with rustc and llvm
#     sh /mnt/apps/myos-src/src/myos/scripts/self-host.sh -r /mnt/apps
#
# It runs `cargo build -p kernel` for the kernel's bare-metal target, as
# the launcher does on the build machine: the kernel, and through
# kernel/build.rs every module and every no_std program (init, netd, ...),
# each at target/<name>-<target>. APPS is where the rustc app is (-r,
# else get-myos's root); CARGO ARGS go to cargo (--release, -j 2).
#
# Then the initramfs, target/initramfs-<arch>.cpio.zst: the running
# system's (/proc/boot/initramfs) with every module and program built here
# in place of its own (user/mkinitramfs, a program of myos this builds
# too). The kernel and it are a boot slot's two files (docs/install.md):
#
#     cp target/<target>/debug/kernel /boot/boot/a/kernel
#     cp target/initramfs-<arch>.cpio.zst /boot/boot/a/initramfs
#
# What is not built here is in that initramfs as the system runs it: the C
# programs and libraries, the std programs.
#
# One program is not built here: `http`, whose TLS is LibreSSL over newlib
# (C that is cross-built on the build machine). It is taken from the
# running system (MYOS_PREBUILT, kernel/build.rs).
set -euo pipefail

apps=
if [ "${1-}" = -r ]; then
	[ $# -ge 2 ] || { echo "usage: self-host.sh [-r APPS] [CARGO ARGS...]" >&2; exit 2; }
	apps=$2
	shift 2
fi
SRC=$(cd "$(dirname "$0")/.." && pwd)

# The kernel's target (soft float) and the programs' (hard float, where
# it is another): rust-toolchain.toml's, kernel/build.rs's `user_target`.
arch=$(uname -m)
case $arch in
x86_64) target=x86_64-unknown-none user= ;;
aarch64) target=aarch64-unknown-none-softfloat user=aarch64-unknown-none ;;
riscv64) target=riscv64imac-unknown-none-elf user=riscv64gc-unknown-none-elf ;;
*) echo "self-host: no kernel for $arch" >&2; exit 1 ;;
esac

[ -d "$SRC/vendor" ] || { echo "self-host: $SRC has no vendor/ (the myos-src package's tree does)" >&2; exit 1; }

# http, as the running system has it.
mkdir -p "$SRC/target"
cp /bin/custom/http "$SRC/target/http-$target"

# The bare-metal links are lld's own: rustc's linker for a myos program
# (the app's rust-lld) adds the C library to what it links.
linkers=
for t in $target $user; do
	linkers="$linkers CARGO_TARGET_$(echo "$t" | tr 'a-z-' 'A-Z_')_LINKER=ld.lld"
done

# RUSTC_BOOTSTRAP: the tree's unstable cargo settings (.cargo/config.toml:
# an artifact dependency, a JSON target) are a nightly's, and the package's
# cargo is not built as one.
cd "$SRC"
echo "self-host: building the kernel for $target in $SRC"
# cargo shows the kernel's crates; the modules and the programs are built
# inside kernel/build.rs, which shows nothing: how far that is, a minute
# at a time.
t0=$(date +%s)
(
	while sleep 60; do
		echo "self-host: $(($(date +%s) - t0)) s, $(ls target | grep -c -- "-$target\$") modules and programs built"
	done
) &
beat=$!
trap 'kill $beat 2> /dev/null' EXIT
# shellcheck disable=SC2086
env $linkers MYOS_PREBUILT=http RUSTC_BOOTSTRAP=1 CARGO_HOME="$SRC/target/cargo-home" \
	run-myos ${apps:+-r "$apps"} rustc:cargo build --offline -p kernel --target "$target" "$@"

profile=debug
for a in "$@"; do
	[ "$a" = --release ] && profile=release
done
echo "self-host: built $SRC/target/$target/$profile/kernel"

# The packer, for the system it runs on, and its archive.
env RUSTC_BOOTSTRAP=1 CARGO_HOME="$SRC/target/cargo-home" \
	run-myos ${apps:+-r "$apps"} rustc:cargo build --offline --release \
	--manifest-path user/mkinitramfs/Cargo.toml --target-dir target/mkinitramfs
target/mkinitramfs/release/mkinitramfs "$SRC" "$arch" /proc/boot/initramfs "target/initramfs-$arch.cpio.zst"
echo "self-host: built $SRC/target/initramfs-$arch.cpio.zst"
