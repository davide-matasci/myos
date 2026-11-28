#!/bin/sh
# self-host.sh [-r APPS] [CARGO ARGS...]
#
# Build this tree's kernel on myos with myos's own compilers: the rustc
# package's cargo and rustc, and the llvm package's clang and lld, all
# native programs. For the arch it runs on, from the crates in vendor/,
# with no network and no Linux layer (docs/userspace.md "Building myos on
# myos").
#
# The tree is the myos-src package's (packages/myos-src: the source the
# system was built from, with its crates, LibreSSL's source and Rust's
# library), installed
# on a disk: the build writes a few GiB under target/.
#
#     mkfs.ext2 /dev/nvme1n1/data && mount /dev/nvme1n1/data /mnt ext2
#     get-myos -r /mnt/apps myos-src        # with rustc, llvm, make, awk and python
#     sh /mnt/apps/myos-src/src/myos/scripts/self-host.sh -r /mnt/apps
#
# First newlib and libgloss (toolchain/newlib/build.sh: newlib's own
# configure and make, with clang; the make, awk and python apps are for
# it), the C library of the C userspace, and the C programs against it
# (scripts/build-c-smokes.sh: user/c, /bin/etc/sec among them). Then
# LibreSSL against it (ports/libressl/build.sh), which `http` links
# through user/tls.
#
# Then the std programs (user/std/build.sh: /bin/std), and before them
# the std they link: Rust's library with myos in it, built from source for
# <arch>-unknown-myos (toolchain/std, `-Z build-std`).
#
# Then `cargo build -p kernel` for the kernel's bare-metal target, as the
# launcher does on the build machine: the kernel, and through
# kernel/build.rs every module and every no_std program (init, netd, http,
# ...), each at target/<name>-<target>. APPS is where the apps are (-r,
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
# What is not built here is in that initramfs as the system runs it:
# mlibc and what links it, the ports.
set -euo pipefail

apps=
if [ "${1-}" = -r ]; then
	[ $# -ge 2 ] || { echo "usage: self-host.sh [-r APPS] [CARGO ARGS...]" >&2; exit 2; }
	apps=$2
	shift 2
fi
SRC=$(cd "$(dirname "$0")/.." && pwd)

# The rest runs in the view of the apps (run-myos): cargo, rustc, clang,
# lld, make, awk and python on PATH. MYOS_SELF_HOST tells
# the tree's scripts where they run (scripts/myos-c-userspace-lib.sh).
if [ -z "${MYOS_SELF_HOST-}" ]; then
	MYOS_SELF_HOST=1 exec run-myos ${apps:+-r "$apps"} rustc,make,awk,python:/bin/sh "$SRC/scripts/self-host.sh" "$@"
fi

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

cd "$SRC"
mkdir -p target

# The builds say little while they run (the C compiles are silent, and
# cargo shows the kernel's crates but not the modules and programs, which
# kernel/build.rs builds): how far each is, a minute at a time.
t0=$(date +%s)
(
	while sleep 60; do
		echo "self-host: $(($(date +%s) - t0)) s, $(find "target/newlib-build-$arch" -name '*.o' 2> /dev/null | grep -c .) objects of newlib, $(find "target/libressl-$arch/obj" -name '*.o' 2> /dev/null | grep -c .) objects of LibreSSL, $(ls target | grep -- "^std-.*-$arch-unknown-myos\$" | grep -vc -- -build-) std programs, $(ls target | grep -c -- "-$target\$") modules and programs built"
	done
) &
beat=$!
trap 'kill $beat 2> /dev/null' EXIT

echo "self-host: building newlib for $arch"
sh toolchain/newlib/build.sh > target/newlib-build.log 2>&1 \
	|| { tail -n 40 target/newlib-build.log >&2; exit 1; }
echo "self-host: newlib built in $(($(date +%s) - t0)) s"

echo "self-host: building the C programs for $arch"
t1=$(date +%s)
sh scripts/build-c-smokes.sh > target/c-smokes-build.log 2>&1 \
	|| { tail -n 40 target/c-smokes-build.log >&2; exit 1; }
echo "self-host: the C programs built in $(($(date +%s) - t1)) s"

echo "self-host: building LibreSSL for $arch"
t1=$(date +%s)
sh ports/libressl/build.sh
echo "self-host: LibreSSL built in $(($(date +%s) - t1)) s"

# cargo's settings for every build below. RUSTC_BOOTSTRAP: the tree's
# unstable ones (.cargo/config.toml: an artifact dependency, a JSON
# target) are a nightly's, and the package's cargo is not built as one.
export RUSTC_BOOTSTRAP=1 CARGO_HOME="$SRC/target/cargo-home" CARGO_NET_OFFLINE=true

echo "self-host: building std and the std programs for $arch-unknown-myos"
t1=$(date +%s)
sh user/std/build.sh
echo "self-host: std and the std programs built in $(($(date +%s) - t1)) s"

# The bare-metal links are lld's own: rustc's linker for a myos program
# (the app's rust-lld) adds the C library to what it links.
linkers=
for t in $target $user; do
	linkers="$linkers CARGO_TARGET_$(echo "$t" | tr 'a-z-' 'A-Z_')_LINKER=ld.lld"
done

echo "self-host: building the kernel for $target in $SRC"
# shellcheck disable=SC2086
env $linkers cargo build -p kernel --target "$target" "$@"

profile=debug
for a in "$@"; do
	[ "$a" = --release ] && profile=release
done
echo "self-host: built $SRC/target/$target/$profile/kernel"

# The packer, for the system it runs on, and its archive.
cargo build --release \
	--manifest-path user/mkinitramfs/Cargo.toml --target-dir target/mkinitramfs
target/mkinitramfs/release/mkinitramfs "$SRC" "$arch" /proc/boot/initramfs "target/initramfs-$arch.cpio.zst"
echo "self-host: built $SRC/target/initramfs-$arch.cpio.zst"
