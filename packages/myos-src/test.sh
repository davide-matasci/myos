# myos-src, installed on the scratch disk with the compiler it needs
# (`disk`, the runner's disk_apps): myos builds what it is made of, with
# its own rustc and no network.
SELF_SRC=$DISK_APPS/myos-src/src/myos
# The kernel's bare-metal target on this arch (scripts/self-host.sh).
self_target() {
	case $(uname -m) in
	x86_64) echo x86_64-unknown-none ;;
	aarch64) echo aarch64-unknown-none-softfloat ;;
	riscv64) echo riscv64imac-unknown-none-elf ;;
	esac
}
# A kernel module, built on myos for the kernel's target from the source
# package: core and alloc for a bare-metal target (the rustc app's), the
# crates from vendor/, lld's link of a position-independent module. On
# every arch; the whole kernel is self_kernel's.
self_module() {
	disk_apps myos-src || return 1
	st_target=$(self_target)
	# kernel/build.rs's flags for the module (`nested_elf`).
	st_flags="-C panic=abort"
	[ "$st_target" = x86_64-unknown-none ] \
		|| st_flags="$st_flags -C relocation-model=pic -C link-arg=-pie -C link-arg=-z -C link-arg=notext"
	st_linker=CARGO_TARGET_$(echo "$st_target" | tr 'a-z-' 'A-Z_')_LINKER
	t0=$(date +%s)
	(cd $SELF_SRC && env "$st_linker=ld.lld" RUSTFLAGS="$st_flags" RUSTC_BOOTSTRAP=1 CARGO_HOME=/tmp/disk/cargo-home \
		run-myos -r $DISK_APPS rustc:cargo build --offline --manifest-path modules/hello/Cargo.toml \
		--target "$st_target" --target-dir /tmp/disk/self-module) >&3 2>&1 || return 1
	echo "self_module: built in $(($(date +%s) - t0)) s" >&3
	st_elf=/tmp/disk/self-module/$st_target/debug/hello
	# An ELF, and one the kernel loads in place of the image's.
	[ "$(dd if=$st_elf bs=1 skip=1 count=3 2> /dev/null)" = ELF ] || { echo "no ELF at $st_elf"; return 1; }
	if grep -q "^hello$" /proc/modules; then
		rmmod hello || return 1
	fi
	insmod $st_elf && grep -q "^hello$" /proc/modules
}
t -t net,heavy,disk,exec,self self_module self_module
# The kernel itself, with every module and no_std program (http among
# them, over the LibreSSL and the newlib the script builds first), the std
# programs over a std built from source, and the initramfs with them in it (scripts/self-host.sh), on x86_64, where the
# test machine runs on the host's CPU: under emulation the build is hours.
self_kernel() {
	disk_apps myos-src || return 1
	t0=$(date +%s)
	sh $SELF_SRC/scripts/self-host.sh -r $DISK_APPS >&3 2>&1 || return 1
	echo "self_kernel: built in $(($(date +%s) - t0)) s" >&3
	sk_elf=$SELF_SRC/target/x86_64-unknown-none/debug/kernel
	[ "$(dd if=$sk_elf bs=1 skip=1 count=3 2> /dev/null)" = ELF ] || { echo "no ELF at $sk_elf"; return 1; }
	# Every module and program the build makes, where the image takes them.
	for sk_f in console-x86_64-unknown-none xhci-x86_64-unknown-none init-x86_64-unknown-none netd-x86_64-unknown-none \
		http-x86_64-unknown-none libressl-x86_64/lib/libtls.a \
		newlib-build-x86_64/x86_64-unknown-myos/newlib/libc.a \
		sec-x86_64-unknown-none battery-x86_64-unknown-none \
		myos-sysroot/lib/rustlib/x86_64-unknown-myos/lib std-thread-x86_64-unknown-myos; do
		[ -s $SELF_SRC/target/$sk_f ] || { echo "no target/$sk_f"; return 1; }
	done
	sk_fs=$SELF_SRC/target/initramfs-x86_64.cpio.zst
	[ -s $sk_fs ] || { echo "no $sk_fs"; return 1; }
	# In place of the booted slot's kernel and initramfs, with their
	# sha256 beside them, and those of files the initramfs now has
	# from this build: the launcher boots the disk again, and what runs in
	# that boot must be these (run.sh selfbuilt). init is the first
	# program of that boot, console its first module; http is the one
	# with C in it, which that boot fetches a page with; thread is a std
	# program, which that boot runs; sec is a C program, which runs
	# /etc/rc in that boot.
	sk_slot=$(sed -n 's/.*slot=\([ab]\).*/\1/p' /proc/cmdline)
	[ -n "$sk_slot" ] || { echo "no slot in $(cat /proc/cmdline)"; return 1; }
	cp $sk_elf /boot/boot/$sk_slot/kernel || return 1
	cp $sk_fs /boot/boot/$sk_slot/initramfs || return 1
	{
		echo "$(sha256sum $sk_elf | cut -d' ' -f1) /proc/boot/kernel"
		echo "$(sha256sum $sk_fs | cut -d' ' -f1) /proc/boot/initramfs"
		echo "$(sha256sum $SELF_SRC/target/init-x86_64-unknown-none | cut -d' ' -f1) /bin/custom/init"
		echo "$(sha256sum $SELF_SRC/target/console-x86_64-unknown-none | cut -d' ' -f1) /lib/modules/console"
		echo "$(sha256sum $SELF_SRC/target/http-x86_64-unknown-none | cut -d' ' -f1) /bin/custom/http"
		echo "$(sha256sum $SELF_SRC/target/std-thread-x86_64-unknown-myos | cut -d' ' -f1) /bin/std/thread"
		echo "$(sha256sum $SELF_SRC/target/sec-x86_64-unknown-none | cut -d' ' -f1) /bin/etc/sec"
	} > /boot/boot/self-built
	[ "$(sha256sum /boot/boot/$sk_slot/kernel | cut -d' ' -f1)" = "$(sha256sum $sk_elf | cut -d' ' -f1)" ] \
		&& [ "$(sha256sum /boot/boot/$sk_slot/initramfs | cut -d' ' -f1)" = "$(sha256sum $sk_fs | cut -d' ' -f1)" ]
}
if [ "$(uname -m)" = x86_64 ]; then
	t -t net,heavy,disk,exec,self -w 1800 self_kernel self_kernel
fi
