# get-myos: the image's release file, and in the full mode (the host serves
# this build's packages) the index's header against it, the listing with
# the installed marks, and the versions of an app: an upgrade beside the
# old version, either one run, a rollback and pruning. The
# dependency install itself is the `packages` test of run.sh, on the fresh
# system. Then the boot disk (docs/install.md): --upgrade writes the
# mirror's kernel and initramfs into slot b of the running boot disk and
# makes it the default (the launcher boots the disk again to check it comes
# up from b), and --install lays a boot disk out on the scratch disk, from
# the mirror and, in every mode, with --local from the running system.
release_file() {
	cat /lib/myos-release
	grep -q '^release=[0-9]* commit=[0-9a-f]* abi=[0-9][0-9]*$' /lib/myos-release
}
t -t smoke release release_file

get_myos_list() {
	get-myos -m $MIRROR -l > /tmp/get-myos-list || return 1
	cat /tmp/get-myos-list
	abi=$(sed 's/.*abi=//' /lib/myos-release)
	grep -q "^mirror: release [0-9]* abi $abi\$" /tmp/get-myos-list || return 1
	grep -q "^system: release [0-9]* abi $abi\$" /tmp/get-myos-list || return 1
	# Every package of the mirror was installed by the packages test, st
	# with its dependencies.
	! grep -q ' -$' /tmp/get-myos-list || return 1
	grep -q '^st [0-9a-f]* x11-xft,x11-fonts installed [0-9a-f]*\*$' /tmp/get-myos-list
}

# lua's state: where ROOT/lua points, its record, the -l line's tail.
lua_is() {
	lua_link=$(readlink $MYOS_APPS/lua) lua_rec=$(cat $MYOS_APPS/.get-myos/pkgs/lua)
	lua_list=$(get-myos -m $MIRROR -l | sed -n 's/^lua [0-9a-f]* - //p')
	echo "lua -> $lua_link, recorded $lua_rec, listed $lua_list"
	[ "$lua_link" = lua-$1 ] && [ "$lua_rec" = $1 ] && [ "$lua_list" = "$2" ]
}
# What lua's view (run-myos $1) has in /lib/lua-marker: the old version's.
lua_marker() {
	run-myos $1:/bin/sh -c 'cat /lib/lua-marker 2> /dev/null || echo none'
}
# Versions (docs/packages.md): lua made to look installed at an older
# version (its directory renamed, a marker added, the pointer and the
# record at it); -u unpacks the index's beside it and flips the pointer,
# keeping the old one; run-myos runs either; --rollback points back; -u
# then flips forward without a download; a third version makes -u remove
# the oldest. (lua's own behaviour is its test's: `lua -v` fails on
# riscv64.)
get_myos_versions() {
	r=$MYOS_APPS old=000000000000 third=111111111111
	want=$(get-myos -m $MIRROR -l | sed -n 's/^lua \([0-9a-f]*\) .*/\1/p')
	lua_is $want "installed $want*" || return 1
	mv $r/lua-$want $r/lua-$old && mkdir -p $r/lua-$old/lib && echo old > $r/lua-$old/lib/lua-marker || return 1
	rm $r/lua && ln -s lua-$old $r/lua && echo $old > $r/.get-myos/pkgs/lua || return 1
	lua_is $old "upgrade $old*" || return 1
	get-myos -m $MIRROR -u > /tmp/versions.out 2>&1 || { cat /tmp/versions.out; return 1; }
	grep -q "^get-myos: lua $want\$" /tmp/versions.out || { cat /tmp/versions.out; return 1; }
	lua_is $want "installed $want*,$old" && [ -x $r/lua/bin/custom/lua ] || return 1
	[ "$(lua_marker lua)" = none ] && [ "$(lua_marker lua@000)" = old ] || return 1
	get-myos --rollback lua || return 1
	lua_is $old "upgrade $old*,$want" && [ "$(lua_marker lua)" = old ] || return 1
	[ "$(lua_marker lua@${want%??????})" = none ] || return 1
	get-myos -m $MIRROR -u > /tmp/versions.out 2>&1 || { cat /tmp/versions.out; return 1; }
	grep -q "^get-myos: lua: back to $want\$" /tmp/versions.out || { cat /tmp/versions.out; return 1; }
	lua_is $want "installed $want*,$old" || return 1
	mkdir $r/lua-$third && cp $r/lua/app $r/lua-$third/ || return 1
	rm $r/lua && ln -s lua-$third $r/lua && echo $third > $r/.get-myos/pkgs/lua || return 1
	get-myos -m $MIRROR -u > /tmp/versions.out 2>&1 || { cat /tmp/versions.out; return 1; }
	lua_is $want "installed $want*,$third" && [ ! -e $r/lua-$old ] || return 1
	rm -r $r/lua-$third
}
# The running boot disk's ESP, which `mount -a` mounted at /boot: its
# partition in $boot_part.
boot_esp() {
	boot_part=$(grep '^/dev/[^ ]* /boot fat ' /proc/mounts | cut -d' ' -f1)
	boot_part=${boot_part#/dev/}
	[ -n "$boot_part" ] && [ "$(cat /boot/boot/a/version)" = "$(cat /lib/myos-release)" ]
}

# Limine on the running boot disk made stale, so that --upgrade has to
# bring it to the release's (issue #372): the EFI binary changed, and on x86
# limine-bios.sys too and the BIOS stage erased (the bios job's second boot
# then needs the one --upgrade writes again).
stale_limine() {
	boot_esp || return 1
	for f in /boot/EFI/BOOT/*; do
		echo stale >> $f
	done
	bios=/boot/boot/limine/limine-bios.sys
	[ -f $bios ] && echo stale >> $bios
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
	stale_limine || return 1
	get-myos -m $MIRROR --upgrade -f > /tmp/upgrade.out 2>&1 || { cat /tmp/upgrade.out; return 1; }
	cat /tmp/upgrade.out
	r=0
	grep -q '^get-myos: Limine: replaced EFI/BOOT/' /tmp/upgrade.out || r=1
	if [ -f /lib/myos-boot/boot-limine-limine-bios.sys ]; then
		grep -q '^get-myos: Limine: replaced boot/limine/limine-bios.sys$' /tmp/upgrade.out \
			&& grep -q '^get-myos: Limine: rewrote the BIOS stage' /tmp/upgrade.out || r=1
	fi
	boot_esp || return 1
	# (Not `name`: run.sh's t reports the test under that variable.)
	while read -r what a b c d; do
		case $what in
		kernel | initramfs) f=boot/b/$what sum=$b ;;
		esp) f=$a sum=$c ;;
		*) continue ;;
		esac
		[ "$f" = boot/limine/limine.conf ] && continue
		got=$(sha256sum /boot/$f | cut -d' ' -f1)
		[ "$got" = "$sum" ] || { echo "$f: $got, list: $sum"; r=1; }
	done < $(get-myos --root)/.get-myos/boot
	head -1 $(get-myos --root)/.get-myos/boot | sed 's/^# myos //' > /tmp/boot-version
	cmp /tmp/boot-version /boot/boot/b/version || r=1
	conf=/boot/boot/limine/limine.conf
	cat $conf
	[ "$(grep -c '^/' $conf)" -eq 2 ] || r=1
	[ "$(grep '^/' $conf | head -1)" = "/myos b" ] || r=1
	[ "$(grep 'path:' $conf | head -1)" = "    path: boot():/boot/b/kernel" ] || r=1
	grep -q '^    cmdline: slot=a$' $conf && grep -q '^timeout: 3$' $conf && grep -q '^serial: yes$' $conf || r=1
	[ $r = 0 ] || return 1
	get-myos -m $MIRROR --upgrade 2>&1 | grep -q 'up to date'
}

# What --install made on the scratch disk $1 from the boot list $2: the
# BIOS boot partition, the ESP and the data partition (the rest of the
# disk); the ESP has slot a and Limine's files as the list has them (the
# limine.conf booting slot a alone), slot a's version is the list's
# release, slot b is empty, its fstab names the data partition (at /data);
# the data partition is an empty ext2.
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
	g=$(grep "^$d/p3 " /proc/partitions | cut -d' ' -f5)
	grep -q "^PARTUUID=$g /data ext2\$" /tmp/new-esp/fstab || { cat /tmp/new-esp/fstab; r=1; }
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
	installed_disk nvme1n1 $(get-myos --root)/.get-myos/boot
}

# --install --local on the scratch disk, no mirror: the running system's
# kernel and initramfs (/proc/boot/) in slot a, at its release, and
# Limine's files from /lib/myos-boot/. In the full mode it runs after the
# mirror's install, last: the host boots the disk it made next (src/main.rs
# `test_boots`).
boot_install_local() {
	scratch_disk
	get-myos --install /dev/nvme1n1 --local || return 1
	list=$(get-myos --root)/.get-myos/local-boot
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

# An app in the default root, the boot disk's data partition (/data/apps:
# the full list's own are in /tmp/apps, MYOS_APPS): installed there, it
# runs, and is still there at the next boot (run.sh reboot).
apps_data() {
	[ "$(MYOS_APPS= get-myos --root)" = /data/apps ] || return 1
	MYOS_APPS= get-myos -m $MIRROR clear || return 1
	ls /data/apps/clear/bin/custom/clear /data/apps/clear/app || return 1
	MYOS_APPS= run-myos clear -T linux > /tmp/apps-data.out || return 1
	[ "$(cat /tmp/apps-data.out)" = "$(printf '\033[H\033[J')" ]
}

t -t net,pkg get_myos_list get_myos_list
t -t net,pkg apps_data apps_data
t -t net,pkg get_myos_versions get_myos_versions
t -t net,pkg,install boot_upgrade boot_upgrade
t_last -t net,pkg,install boot_install
t_last -t install boot_install_local
