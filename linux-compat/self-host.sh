# self-host.sh DIR [REV]
#
# Build the myos kernel inside myos (x86_64): Alpine's Rust toolchain under
# the Linux layer, compiling the myos source fetched from GitHub. DIR is
# where it all goes (a mounted disk: the toolchain, the source and the build
# take several GiB):
#
#     mkfs.ext2 /dev/nvme1n1 && mount /dev/nvme1n1 /disk ext2
#     sh /lib/self-host.sh /disk
#
# Each step is skipped when its result is already there, so a second run
# goes on where the first stopped. REV is the branch to build (master).
#
# What it does not build yet: the userland the kernel embeds (the std demo
# programs, c-hello, oksh, getty and login) is copied from this system, and
# the TLS library of the embedded `http` (mbedtls over newlib) is not built
# here at all (docs/linux-compat.md, "Building myos in myos").

D=$1
REV=${2:-master}
if [ -z "$D" ]; then
	echo "usage: sh /lib/self-host.sh DIR [REV]"
	exit 2
fi
R=$D/alpine
SRC=/src/myos
ARCH=x86_64
TARGET=$ARCH-unknown-none

say() {
	echo "self-host: $*"
}

# 1. Alpine's Rust (stable, with its library sources for build-std), lld
# for the kernel's link, clang and bash for the C bits and port scripts,
# git, and busybox's tools (get-alpine runs no install scripts, so its
# links are made here).
if [ ! -x $R/usr/bin/cargo ]; then
	say "installing the toolchain into $R"
	get-alpine -r $R rust cargo rust-src lld clang bash busybox git || exit 1
fi
if [ ! -e $R/usr/bin/env ]; then
	linux --root $R /bin/busybox --install -s || exit 1
fi

# 2. The source.
if [ ! -d $R$SRC/.git ]; then
	say "cloning myos ($REV)"
	mkdir -p $R/src
	linux --root $R git clone --depth 1 -b $REV https://github.com/davide-matasci/myos $SRC || exit 1
fi

# 3. The userland the kernel embeds, from this system (building it needs
# newlib and the patched std sysroot).
T=$R$SRC/target
mkdir -p $T
cp /bin/std/hello $T/std-hello-$ARCH-unknown-myos
cp /bin/std/cat $T/std-cat-$ARCH-unknown-myos
cp /bin/std/echo $T/std-echo-$ARCH-unknown-myos
cp /bin/std/bigalloc $T/std-bigalloc-$ARCH-unknown-myos
cp /bin/etc/hello $T/c-hello-$TARGET
cp /bin/custom/sh $T/oksh-$TARGET
cp /bin/ubase/getty $T/ubase-getty-$TARGET
cp /bin/ubase/login $T/ubase-login-$TARGET
echo "getty:$SRC/target/ubase-getty-$TARGET" > $T/ubase-manifest-$ARCH.txt
echo "login:$SRC/target/ubase-login-$TARGET" >> $T/ubase-manifest-$ARCH.txt

# 4. The kernel. A stable rustc: RUSTC_BOOTSTRAP for the unstable
# features, core and alloc built from source for the bare-metal target
# (Alpine ships the library for its own target only), and lld for the
# link (there is no rust-lld). The build scripts and proc macros are linked
# by clang, not by gcc: Alpine's gcc is not position-independent, and the
# Linux layer runs only position-independent dynamic programs. The
# environment is the Linux one: cargo and the build scripts find their
# tools through PATH.
say "building the kernel"
linux --root $R /usr/bin/env PATH=/usr/bin:/bin:/usr/sbin:/sbin HOME=/root RUSTC_BOOTSTRAP=1 \
	CARGO_UNSTABLE_BUILD_STD=core,alloc \
	CARGO_UNSTABLE_BUILD_STD_FEATURES=compiler-builtins-mem \
	CARGO_TARGET_X86_64_UNKNOWN_NONE_LINKER=ld.lld \
	CARGO_TARGET_X86_64_ALPINE_LINUX_MUSL_LINKER=clang CC=clang \
	cargo build --release --config $SRC/.cargo/config.toml --manifest-path $SRC/Cargo.toml \
	-p kernel --target $TARGET || exit 1
say "built $R$SRC/target/$TARGET/release/kernel"
