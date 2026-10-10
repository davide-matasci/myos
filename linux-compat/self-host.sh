# self-host.sh DIR [REV]
#
# Build the myos kernel inside myos (x86_64): Alpine's Rust toolchain under
# the Linux layer, compiling the myos source fetched from GitHub. DIR is
# where it all goes (a mounted disk: the toolchain, the source and the build
# take several GiB):
#
#     mkfs.ext2 /dev/nvme1n1/data && mount /dev/nvme1n1/data /mnt ext2
#     sh /lib/self-host.sh /mnt
#
# Each step is skipped when its result is already there, so a second run
# goes on where the first stopped. REV is the branch to build (master).
#
# What it does not build yet is copied from this system: the C and std
# userland the kernel embeds (the std demo programs, c-hello, oksh, getty
# and login) and `http`, whose TLS library is mbedtls over newlib.
# docs/linux-compat.md, "Building myos in myos", lists what is missing.

D=$1
REV=${2:-master}
if [ -z "$D" ]; then
	echo "usage: sh /lib/self-host.sh DIR [REV]"
	exit 2
fi
R=$D/alpine
SRC=/src/myos
URL=https://github.com/davide-matasci/myos
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
	get-myos get-alpine && run-myos get-alpine -r $R rust cargo rust-src lld clang bash busybox git || exit 1
fi
if [ ! -e $R/usr/bin/env ]; then
	linux --root $R /bin/busybox --install -s || exit 1
fi

# 2. The source: cloned, or brought to REV's latest commit.
if [ ! -d $R$SRC/.git ]; then
	say "cloning myos ($REV)"
	mkdir -p $R/src
	linux --root $R git clone --depth 1 -b $REV $URL $SRC || exit 1
	linux --root $R git -C $SRC config core.fileMode false || exit 1
else
	say "updating myos ($REV)"
	# myos keeps no permission bits (every ext2 file reads as 0755): git
	# would take each one for a mode change.
	linux --root $R git -C $SRC config core.fileMode false || exit 1
	linux --root $R git -C $SRC fetch --depth 1 $URL $REV || exit 1
	linux --root $R git -C $SRC checkout -q FETCH_HEAD || exit 1
fi

# 3. The userland the kernel embeds, from this system (building it needs
# newlib and the patched std sysroot; `http` needs mbedtls over newlib, and
# MYOS_PREBUILT below has kernel/build.rs take it as it is).
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
cp /bin/custom/http $T/http-$TARGET
echo "getty:$SRC/target/ubase-getty-$TARGET" > $T/ubase-manifest-$ARCH.txt
echo "login:$SRC/target/ubase-login-$TARGET" >> $T/ubase-manifest-$ARCH.txt

# A command in the Alpine root, with the Linux environment (cargo and the
# build scripts find their tools through PATH), after VAR=value arguments. A stable rustc: RUSTC_BOOTSTRAP
# for the unstable features, and lld for the bare-metal links (there is no
# rust-lld). The build scripts and proc macros are linked by clang, not by
# gcc: Alpine's gcc is not position-independent, and the Linux layer runs
# only position-independent dynamic programs.
alpine() {
	linux --root $R /usr/bin/env PATH=/usr/bin:/bin:/usr/sbin:/sbin HOME=/root RUSTC_BOOTSTRAP=1 \
		CARGO_TARGET_X86_64_UNKNOWN_NONE_LINKER=ld.lld \
		CARGO_TARGET_X86_64_ALPINE_LINUX_MUSL_LINKER=clang CC=clang "$@"
}

# 4. core, alloc and compiler_builtins for the bare-metal target, built once
# from Alpine's library sources (Alpine ships them built for its own target
# only) and installed next to that target's, as rustup would: the kernel's
# build.rs builds every module and user program in a target directory of
# its own, and each would rebuild them (a quarter of an hour apiece here).
L=$R/usr/lib/rustlib/$TARGET/lib
if ! ls $L/libcore-*.rlib > /dev/null 2>&1; then
	say "building core and alloc for $TARGET"
	S=/tmp/myos-sysroot
	mkdir -p $R$S
	printf '[package]\nname = "sysroot"\nversion = "0.0.0"\nedition = "2021"\n[lib]\npath = "lib.rs"\n' > $R$S/Cargo.toml
	echo '#![no_std]' > $R$S/lib.rs
	alpine CARGO_UNSTABLE_BUILD_STD=core,alloc CARGO_UNSTABLE_BUILD_STD_FEATURES=compiler-builtins-mem \
		cargo build --release --manifest-path $S/Cargo.toml --target $TARGET || exit 1
	mkdir -p $L
	for f in $R$S/target/$TARGET/release/deps/lib*.rlib; do
		case $f in */libsysroot-*) ;; *) cp $f $L/ ;; esac
	done
	rm -rf $R$S
fi

# 5. The kernel.
say "building the kernel"
alpine MYOS_PREBUILT=http cargo build --release --config $SRC/.cargo/config.toml \
	--manifest-path $SRC/Cargo.toml -p kernel --target $TARGET || exit 1
say "built $R$SRC/target/$TARGET/release/kernel"
