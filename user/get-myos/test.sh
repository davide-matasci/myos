# get-myos: the image's release file, and in the full mode (the host serves
# this build's packages) the index's header against it, the listing with
# the installed marks, and an upgrade: a package recorded at a stale
# version is fetched again by -u and recorded at the index's. The
# dependency install itself is the `packages` test of run.sh, on the fresh
# system. Then the boot disk (docs/install.md): --upgrade writes the
# mirror's kernel and initramfs into slot b of the running boot disk and
# makes it the default (the launcher boots the disk again to check it comes
# up from b), and --install lays a boot disk out on the scratch disk.
release_file() {
	cat /lib/myos-release
	grep -q '^release=[0-9]* commit=[0-9a-f]* abi=[0-9][0-9]*$' /lib/myos-release
}
t release release_file

get_myos_list() {
	get-myos -m $MIRROR -l > /tmp/get-myos-list || return 1
	cat /tmp/get-myos-list
	abi=$(sed 's/.*abi=//' /lib/myos-release)
	grep -q "^mirror: release [0-9]* abi $abi\$" /tmp/get-myos-list || return 1
	grep -q "^system: release [0-9]* abi $abi\$" /tmp/get-myos-list || return 1
	# Every package of the mirror was installed by the packages test, st
	# with its dependencies.
	! grep -q ' -$' /tmp/get-myos-list || return 1
	grep -q '^st [0-9a-f]* x11-xft,x11-fonts installed$' /tmp/get-myos-list
}
get_myos_upgrade() {
	echo stale > /tmp/pkg/var/lib/get-myos/pkgs/lua
	get-myos -m $MIRROR -u || return 1
	have=$(cat /tmp/pkg/var/lib/get-myos/pkgs/lua)
	want=$(get-myos -m $MIRROR -l | sed -n 's/^lua \([0-9a-f]*\) .*/\1/p')
	echo "lua: recorded $have, index $want"
	# The record follows the index and the binary is bound again (its
	# behaviour is lua's own business: `lua -v` fails on riscv64).
	[ -n "$want" ] && [ "$have" = "$want" ] && [ -x /bin/custom/lua ]
}
# The ESP of the running boot disk mounted at $1: the one whose slot a has
# this system's release; its partition in $boot_part.
mount_boot_esp() {
	for p in $(grep ' c12a7328-f81f-11d2-ba4b-00a0c93ec93b ' /proc/partitions | cut -d' ' -f1); do
		mount /dev/$p $1 fat || continue
		boot_part=$p
		[ "$(cat $1/boot/a/version 2> /dev/null)" = "$(cat /lib/myos-release)" ] && return 0
		umount $1
	done
	return 1
}

# Limine on the running boot disk made stale, so that --upgrade has to
# bring it to the release's (issue #372): the EFI binary changed, and on x86
# limine-bios.sys too and the BIOS stage erased (the bios job's second boot
# then needs the one --upgrade writes again).
stale_limine() {
	mount_boot_esp /tmp/boot-esp || return 1
	for f in /tmp/boot-esp/EFI/BOOT/*; do
		echo stale >> $f
	done
	bios=/tmp/boot-esp/boot/limine/limine-bios.sys
	[ -f $bios ] && echo stale >> $bios
	umount /tmp/boot-esp
	if [ -f /lib/myos-boot/boot-limine-limine-bios.sys ]; then
		dd if=/dev/zero of=/dev/${boot_part%/*}/p1 bs=4096 count=256 2> /dev/null || return 1
	fi
}

# --upgrade -f (the mirror is this build: not newer) of a disk whose Limine
# is stale: slot b gets the mirror's files, which the disk holds (their
# SHA-256 against the list), and its version; Limine's files are replaced
# by the release's (on x86 with the BIOS stage); limine.conf boots b by
# default, a as the fallback, the global lines the release's. A second run
# without -f has nothing to do.
boot_upgrade() {
	grep -q 'slot=a' /proc/cmdline || { cat /proc/cmdline; return 1; }
	mkdir -p /tmp/boot-esp && stale_limine || return 1
	get-myos -m $MIRROR --upgrade -f > /tmp/upgrade.out 2>&1 || { cat /tmp/upgrade.out; return 1; }
	cat /tmp/upgrade.out
	r=0
	grep -q '^get-myos: Limine: replaced EFI/BOOT/' /tmp/upgrade.out || r=1
	if [ -f /lib/myos-boot/boot-limine-limine-bios.sys ]; then
		grep -q '^get-myos: Limine: replaced boot/limine/limine-bios.sys$' /tmp/upgrade.out \
			&& grep -q '^get-myos: Limine: rewrote the BIOS stage' /tmp/upgrade.out || r=1
	fi
	mount_boot_esp /tmp/boot-esp || return 1
	# (Not `name`: run.sh's t reports the test under that variable.)
	while read -r what a b c d; do
		case $what in
		kernel | initramfs) f=boot/b/$what sum=$b ;;
		esp) f=$a sum=$c ;;
		*) continue ;;
		esac
		[ "$f" = boot/limine/limine.conf ] && continue
		got=$(sha256sum /tmp/boot-esp/$f | cut -d' ' -f1)
		[ "$got" = "$sum" ] || { echo "$f: $got, list: $sum"; r=1; }
	done < /tmp/pkg/var/lib/get-myos/boot
	head -1 /tmp/pkg/var/lib/get-myos/boot | sed 's/^# myos //' > /tmp/boot-version
	cmp /tmp/boot-version /tmp/boot-esp/boot/b/version || r=1
	conf=/tmp/boot-esp/boot/limine/limine.conf
	cat $conf
	[ "$(grep -c '^/' $conf)" -eq 2 ] || r=1
	[ "$(grep '^/' $conf | head -1)" = "/myos b" ] || r=1
	[ "$(grep 'path:' $conf | head -1)" = "    path: boot():/boot/b/kernel" ] || r=1
	grep -q '^    cmdline: slot=a$' $conf && grep -q '^timeout: 3$' $conf && grep -q '^serial: yes$' $conf || r=1
	umount /tmp/boot-esp
	[ $r = 0 ] || return 1
	get-myos -m $MIRROR --upgrade 2>&1 | grep -q 'up to date'
}

# What --install made on the scratch disk $1 from the boot list $2: the
# BIOS boot partition, the ESP and the data partition (the rest of the
# disk); the ESP has slot a and Limine's files as the list has them (the
# limine.conf booting slot a alone), slot a's version is the list's
# release, slot b is empty; the data partition is an empty ext2.
installed_disk() {
	d=$1
	grep "^$d/" /proc/partitions
	grep -q "^$d/p1 2048 1048576 21686148-6449-6e6f-744e-656564454649 .* \"BIOS Boot\"$" /proc/partitions || return 1
	grep -q "^$d/p2 4096 536870912 c12a7328-f81f-11d2-ba4b-00a0c93ec93b .* \"EFI System\"$" /proc/partitions || return 1
	grep -q "^$d/p3 1052672 [0-9]* 0fc63daf-8483-4772-8e79-3d69d8477de4 .* \"myos data\"$" /proc/partitions || return 1
	mkdir -p /tmp/new-esp /tmp/new-data && mount /dev/$d/p2 /tmp/new-esp fat || return 1
	r=0
	while read -r what a b c dd; do
		case $what in
		kernel | initramfs) f=boot/a/$what sum=$b ;;
		esp) f=$a sum=$c ;;
		*) continue ;;
		esac
		got=$(sha256sum /tmp/new-esp/$f | cut -d' ' -f1)
		[ "$got" = "$sum" ] || { echo "$f: $got, list: $sum"; r=1; }
	done < $2
	head -1 $2 | sed 's/^# myos //' > /tmp/boot-version
	cmp /tmp/boot-version /tmp/new-esp/boot/a/version || r=1
	conf=/tmp/new-esp/boot/limine/limine.conf
	cat $conf
	[ "$(grep -c '^/' $conf)" -eq 1 ] && grep -q '^    cmdline: slot=a$' $conf && grep -q '^timeout: 0$' $conf || r=1
	[ -d /tmp/new-esp/boot/b ] && [ -z "$(ls /tmp/new-esp/boot/b)" ] || { echo "slot b not empty"; r=1; }
	umount /tmp/new-esp
	mount /dev/$d/p3 /tmp/new-data ext2 && [ "$(ls /tmp/new-data)" = lost+found ] && umount /tmp/new-data \
		|| { echo "the data partition is not an empty ext2"; r=1; }
	return $r
}

# The scratch disk (4 GiB) free for --install: nothing of it mounted.
scratch_disk() {
	for m in $(grep "^/dev/nvme1n1/" /proc/mounts | cut -d' ' -f2); do umount $m; done
}

# --install on the scratch disk from the mirror, as installed_disk checks;
# the running boot disk is refused.
boot_install() {
	scratch_disk
	boot=$(grep ' "EFI System"$' /proc/partitions | cut -d/ -f1 | head -1)
	get-myos -m $MIRROR --install $boot 2>&1 | grep -q 'running boot disk' || return 1
	get-myos -m $MIRROR --install /dev/nvme1n1 || return 1
	installed_disk nvme1n1 /tmp/pkg/var/lib/get-myos/boot
}

# --install --local on the scratch disk, no mirror: the running system's
# kernel and initramfs (/proc/boot/) in slot a, at its release, and
# Limine's files from /lib/myos-boot/. In the full mode it runs after the
# mirror's install, last: the host boots the disk it made next (src/main.rs
# `test_boots`).
boot_install_local() {
	scratch_disk
	get-myos --install /dev/nvme1n1 --local || return 1
	list=/tmp/pkg/var/lib/get-myos/local-boot
	cat $list
	[ "$(head -1 $list)" = "# myos $(cat /lib/myos-release)" ] || return 1
	grep -q '^kernel [0-9]* [0-9a-f]* /proc/boot/kernel$' $list \
		&& grep -q '^initramfs [0-9]* [0-9a-f]* /proc/boot/initramfs$' $list || return 1
	installed_disk nvme1n1 $list || return 1
	mount /dev/nvme1n1/p2 /tmp/new-esp fat || return 1
	cmp /proc/boot/kernel /tmp/new-esp/boot/a/kernel && cmp /proc/boot/initramfs /tmp/new-esp/boot/a/initramfs
	r=$?
	umount /tmp/new-esp
	return $r
}

if [ "$MODE" = full ]; then
	t get_myos_list get_myos_list
	t get_myos_upgrade get_myos_upgrade
	t boot_upgrade boot_upgrade
	t_last boot_install
fi
t_last boot_install_local
