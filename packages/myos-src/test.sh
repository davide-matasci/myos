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
# The kernel itself, with every module and no_std program
# (scripts/self-host.sh), on x86_64, where the test machine runs on the
# host's CPU: under emulation the build is hours.
self_kernel() {
	disk_apps myos-src || return 1
	t0=$(date +%s)
	sh $SELF_SRC/scripts/self-host.sh -r $DISK_APPS >&3 2>&1 || return 1
	echo "self_kernel: built in $(($(date +%s) - t0)) s" >&3
	sk_elf=$SELF_SRC/target/x86_64-unknown-none/debug/kernel
	[ "$(dd if=$sk_elf bs=1 skip=1 count=3 2> /dev/null)" = ELF ] || { echo "no ELF at $sk_elf"; return 1; }
	# Every module and program the build makes, where the image takes them.
	for sk_f in console-x86_64-unknown-none xhci-x86_64-unknown-none init-x86_64-unknown-none netd-x86_64-unknown-none; do
		[ -s $SELF_SRC/target/$sk_f ] || { echo "no target/$sk_f"; return 1; }
	done
	# In place of the booted slot's kernel, with its sha256 beside it: the
	# launcher boots the disk again, and that boot's kernel must be this
	# one (run.sh selfbuilt).
	sk_slot=$(sed -n 's/.*slot=\([ab]\).*/\1/p' /proc/cmdline)
	[ -n "$sk_slot" ] || { echo "no slot in $(cat /proc/cmdline)"; return 1; }
	cp $sk_elf /boot/boot/$sk_slot/kernel || return 1
	sha256sum $sk_elf | cut -d' ' -f1 > /boot/boot/self-built
	[ "$(sha256sum /boot/boot/$sk_slot/kernel | cut -d' ' -f1)" = "$(cat /boot/boot/self-built)" ]
}
if [ "$(uname -m)" = x86_64 ]; then
	t -t net,heavy,disk,exec,self self_kernel self_kernel
fi
