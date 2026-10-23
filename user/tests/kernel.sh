# The kernel's own tests: what has no port directory of its own. The exec
# limits, #! scripts, the console's CR, and the Linux compatibility layer
# (docs/linux-compat.md): its module is in every image and loaded at boot
# when the image was built with the feature (`--features linux_compat`),
# with the musl test programs.

# exec limits: 40 arguments and a 711-byte environment string through oksh
# (libgloss execve) into sbase programs.
exec_limits() {
	A="a b c d e f g h"
	A="$A $A $A $A $A"
	X=$A$A$A$A$A$A$A$A$A
	set -- $(/bin/sbase/echo $A)
	Y=$(X=$X /bin/sbase/printenv X)
	echo "args=$# env=${#Y}"
	[ $# -eq 40 ] && [ ${#Y} -eq 711 ]
}
t -t exec,smoke exec_limits exec_limits

# #! scripts: exec runs the interpreter with the script's path in front of
# its arguments, and the line's one argument before that; a missing
# interpreter fails the exec. sbase env execs them itself (the shell would
# fall back to running them on its own).
exec_script() {
	printf '#!/bin/sh\necho "$0 $*"\n' > /tmp/script1
	printf '#!/bin/sh -e\nfalse\necho not reached\n' > /tmp/script2
	printf '#!/nonexistent/sh\necho no\n' > /tmp/script3
	out=$(/bin/sbase/env /tmp/script1 a b)
	echo "script1: $out"
	[ "$out" = "/tmp/script1 a b" ] || return 1
	out=$(/bin/sbase/env /tmp/script2)
	echo "script2: $out"
	[ -z "$out" ] || return 1
	! /bin/sbase/env /tmp/script3 > /dev/null 2>&1
}
t -t exec,smoke exec_script exec_script

# One open file description per open, shared by a fork's and a dup's copies
# of the fd (POSIX): a child's writes advance the offset the parent writes
# at next, so a redirected group's output is complete and in order, through
# the shell's own fd too.
fd_offsets() {
	{ echo one; /bin/sbase/echo two; echo three; } > /tmp/fdo.txt
	cat /tmp/fdo.txt
	[ "$(cat /tmp/fdo.txt)" = "one
two
three" ] || return 1
	exec 4> /tmp/fdd.txt
	echo a >&4
	/bin/sbase/echo b >&4
	echo c >&4
	exec 4>&-
	cat /tmp/fdd.txt
	[ "$(cat /tmp/fdd.txt)" = "a
b
c" ]
}
t -t fs,smoke fd_offsets fd_offsets

# An fd holds its file, not its name: it follows a rename (of the file or
# of a directory above it), a new file that takes the old name is not the
# one it reads or writes, and a file unlinked under it stays readable,
# under a name no listing shows, until it is closed.
fd_identity() {
	echo one > /tmp/fdi.a
	exec 4< /tmp/fdi.a 5>> /tmp/fdi.a
	mv /tmp/fdi.a /tmp/fdi.b
	echo new > /tmp/fdi.a
	echo two >&5
	got=$(cat <&4)
	exec 4<&- 5>&-
	echo "renamed: $got"
	[ "$got" = "one
two" ] && [ "$(cat /tmp/fdi.a)" = new ] || return 1
	mkdir /tmp/fdi.d
	echo three > /tmp/fdi.d/f
	exec 4< /tmp/fdi.d/f
	mv /tmp/fdi.d /tmp/fdi.e
	got=$(cat <&4)
	exec 4<&-
	echo "directory renamed: $got"
	[ "$got" = three ] || return 1
	exec 4< /tmp/fdi.b
	rm /tmp/fdi.b /tmp/fdi.a /tmp/fdi.e/f
	rmdir /tmp/fdi.e
	listed=$(ls -a /tmp | grep -c unlinked)
	got=$(cat <&4)
	exec 4<&-
	echo "unlinked: $got, listed $listed"
	[ "$got" = "one
two" ] && [ "$listed" = 0 ]
}
t -t fs,smoke fd_identity fd_identity

# rmmod: a module that provides nothing (hello) unloads and loads again;
# one with a registration (the block driver's disks) is refused and keeps
# working.
rmmod_hello() {
	grep -q "^hello$" /proc/modules && rmmod hello && ! grep -q "^hello$" /proc/modules \
		&& insmod /lib/modules/hello && grep -q "^hello$" /proc/modules
}
rmmod_busy() {
	! rmmod virtio_blk && grep -q "^virtio_blk$" /proc/modules && ls /dev/vda/data
}
# A /proc/pci rescan re-probes the drivers: the disks are the same ones
# after it, and still readable. (A sector, not the disk's tail: tail reads
# all of it, and on aarch64 and riscv64 vda is the 579 MiB boot disk.)
pci_rescan() {
	ls /dev > /tmp/dev-before.txt
	echo rescan > /proc/pci || return 1
	ls /dev > /tmp/dev-after.txt
	cmp /tmp/dev-before.txt /tmp/dev-after.txt \
		&& [ "$(dd if=/dev/vda/data bs=512 count=1 2> /dev/null | wc -c)" -eq 512 ]
}
# GPT partitions: the launcher's scratch disk (nvme1n1) carries a GPT with a
# 20 MiB FAT16 of ESP type in entry 1, entry 2 empty and 64 MiB of Linux
# data in entry 3: /dev/nvme1n1/p1 and p3, listed in /proc/partitions. A
# partition reads no further than its end; the FAT mounts from p1, an ext2
# made on p3 works and leaves p1 as it was. A rescan keeps a mounted
# partition; with the primary header wiped, the backup at the end of the
# disk gives the same partitions; mkfs.fat makes p3 a FAT32 volume. (The
# port tests reuse the whole disk.)
partitions() {
	d=/dev/nvme1n1
	[ "$(ls $d | tr '\n' ' ')" = "data info p1 p3 " ] || { ls $d; return 1; }
	[ -b $d/p1 ] && [ -b $d/p3 ] || return 1
	cat > /tmp/parts.want <<-EOF
	nvme1n1/p1 2048 20971520 c12a7328-f81f-11d2-ba4b-00a0c93ec93b 61726373-6374-4068-8000-000000000001 "EFI system"
	nvme1n1/p3 65536 67108864 0fc63daf-8483-4772-8e79-3d69d8477de4 61726373-6374-4068-8000-000000000003 "scratch"
	EOF
	grep '^nvme1n1/' /proc/partitions > /tmp/parts.got
	cmp -s /tmp/parts.want /tmp/parts.got || { cat /proc/partitions; return 1; }
	[ "$(dd if=$d/p1 bs=512 skip=40959 count=1 2> /dev/null | wc -c)" -eq 512 ] || return 1
	[ "$(dd if=$d/p1 bs=512 skip=40960 count=1 2> /dev/null | wc -c)" -eq 0 ] || return 1
	mkdir -p /tmp/esp /tmp/p3 && mount $d/p1 /tmp/esp fat || return 1
	[ "$(cat /tmp/esp/msg)" = scratch-esp ] && umount /tmp/esp || return 1
	mkfs.ext2 $d/p3 > /dev/null && mount $d/p3 /tmp/p3 ext2 || return 1
	echo kept > /tmp/p3/k && umount /tmp/p3 && mount $d/p3 /tmp/p3 ext2 || return 1
	[ "$(cat /tmp/p3/k)" = kept ] || return 1
	echo rescan > /proc/pci && [ "$(cat /tmp/p3/k)" = kept ] && umount /tmp/p3 || return 1
	mount $d/p1 /tmp/esp fat && [ "$(cat /tmp/esp/msg)" = scratch-esp ] && umount /tmp/esp || return 1
	dd if=/dev/zero of=$d/data bs=512 seek=1 count=1 conv=notrunc 2> /dev/null || return 1
	echo rescan > /proc/pci || return 1
	grep '^nvme1n1/' /proc/partitions > /tmp/parts.got
	cmp -s /tmp/parts.want /tmp/parts.got || { echo "no backup GPT"; cat /proc/partitions; return 1; }
	mount $d/p3 /tmp/p3 ext2 && [ "$(cat /tmp/p3/k)" = kept ] && umount /tmp/p3 || return 1
	# mkfs.fat: p3 becomes an empty FAT32 volume that keeps a file and a
	# directory across a remount.
	mkfs.fat $d/p3 && mount $d/p3 /tmp/p3 fat || return 1
	[ -z "$(ls /tmp/p3)" ] || { ls /tmp/p3; return 1; }
	mkdir "/tmp/p3/A dir" && cp /bin/sbase/ls "/tmp/p3/A dir/ls" && umount /tmp/p3 || return 1
	mount $d/p3 /tmp/p3 fat && cmp /bin/sbase/ls "/tmp/p3/A dir/ls" && umount /tmp/p3
}
# The boot disk's layout (docs/install.md; a virtio disk on every arch,
# vdc on x86), as `mount -a` mounted it at boot: the ESP at /boot (the
# partition /proc/boot/partuuid names) holds the kernel, the initramfs and
# the version in slot a, an empty slot b, the one limine.conf, which boots
# slot a and tells the kernel so (/proc/cmdline), and fstab, which
# names the data partition: an ext2 at /data, which holds only what the
# boot script put there (boot_rc).
boot_disk() {
	data=$(grep '"myos data"$' /proc/partitions | cut -d' ' -f1)
	[ -n "$data" ] || { cat /proc/partitions; return 1; }
	disk=${data%/*}
	esp=$(grep "^$disk/p[0-9]* .* c12a7328-f81f-11d2-ba4b-00a0c93ec93b " /proc/partitions | cut -d' ' -f1)
	[ -n "$esp" ] || { cat /proc/partitions; return 1; }
	cat /proc/mounts /boot/fstab
	r=0
	grep -q "^$esp .* $(cat /proc/boot/partuuid) " /proc/partitions || r=1
	grep -q "^/dev/$esp /boot fat " /proc/mounts && grep -q "^/dev/$data /data ext2 " /proc/mounts || r=1
	grep -q "^PARTUUID=$(grep "^$data " /proc/partitions | cut -d' ' -f5) /data ext2$" /boot/fstab || r=1
	/bin/sbase/find /boot > /tmp/boot-esp.txt
	[ -s /boot/boot/a/kernel ] && [ -s /boot/boot/a/initramfs ] || r=1
	[ -d /boot/boot/b ] && [ -z "$(ls /boot/boot/b)" ] || r=1
	grep -q '^    path: boot():/boot/a/kernel$' /boot/boot/limine/limine.conf || r=1
	[ "$(cat /boot/boot/a/version)" = "$(cat /lib/myos-release)" ] || r=1
	grep -q 'slot=a' /proc/cmdline || r=1
	[ "$(/bin/sbase/find /boot -name limine.conf | wc -l)" -eq 1 ] || r=1
	# What this boot came from (/proc/boot/) is slot a's, and Limine's files
	# the initramfs carries for `get-myos --install --local` are the ESP's.
	cmp /proc/boot/kernel /boot/boot/a/kernel && cmp /proc/boot/initramfs /boot/boot/a/initramfs || r=1
	while read -r what path size sum file; do
		cmp "$file" "/boot/$path" || r=1
	done < /lib/myos-boot/boot.txt
	[ $r = 0 ] || { cat /tmp/boot-esp.txt; return 1; }
	[ "$(ls /data | tr '\n' ' ')" = "dropbear etc lost+found " ]
}
# The boot script (etc/rc, docs/install.md), which run.sh waited for: as
# root, it bound the data partition's /data/dropbear over /etc/dropbear,
# copied the image's rc.local to /data/etc and ran it, which loaded the
# Linux layer and started dropbear on port 2222.
boot_rc() {
	cat /proc/mounts
	grep -q '^/data/dropbear /etc/dropbear bind' /proc/mounts || return 1
	cmp /etc/rc.local /data/etc/rc.local || return 1
	grep -q "^linux$" /proc/modules || return 1
	cat /tmp/boot.done
	grep -q '^\[ OK \] boot complete (/data/etc/rc.local)$' /tmp/boot.done || return 1
	ps | grep -q ' dropbear$' || { ps; return 1; }
}
# /boot/fstab edited on the running system (docs/install.md): a line naming
# the scratch disk's p3 (FAT since `partitions`) by its unique GUID mounts it
# under /mnt, its mount point made there (/mnt is /tmp/mnt, bound over it);
# a line naming a /dev path is refused and one whose partition is not there
# skipped, without stopping the others; a second `mount -a` leaves what is
# mounted. `mount PARTUUID=...` mounts by GUID too. The fstab is restored.
fstab_mount() {
	cp /boot/fstab /tmp/fstab.orig || return 1
	g=$(grep '^nvme1n1/p3 ' /proc/partitions | cut -d' ' -f5)
	{
		echo "/dev/nvme1n1/p3 /mnt/bad fat"
		echo "PARTUUID=00000000-0000-4000-8000-00000000dead /mnt/gone fat"
		echo "PARTUUID=$g /mnt/scratch/p3 fat rw"
	} >> /boot/fstab
	mount -a > /tmp/mount-a.out 2>&1
	rc=$?
	cp /tmp/fstab.orig /boot/fstab
	cat /tmp/mount-a.out /proc/mounts
	[ $rc = 1 ] || return 1
	grep -q 'PARTUUID=<guid>' /tmp/mount-a.out && grep -q 'dead, skipped$' /tmp/mount-a.out || return 1
	grep -q "^/dev/nvme1n1/p3 /tmp/mnt/scratch/p3 fat " /proc/mounts && [ -d "/mnt/scratch/p3/A dir" ] || return 1
	mount -a || return 1
	[ "$(grep -c '^/dev/nvme1n1/p3 ' /proc/mounts)" -eq 1 ] || return 1
	umount /mnt/scratch/p3 && mkdir -p /mnt/x && mount PARTUUID=$g /mnt/x fat && [ -d "/mnt/x/A dir" ] && umount /mnt/x
}
t -t modules rmmod_hello rmmod_hello
t -t modules rmmod_busy rmmod_busy
t -t pci pci_rescan pci_rescan
# The virtio-scsi disk (modules/virtio_scsi): the launcher gives every boot
# a virtio-scsi host with one 1 MiB disk, the first `sd` disk (the USB
# stick comes later, on the USB thread). It reads and writes by sector,
# and ends where READ CAPACITY said.
scsi_disk() {
	d=/dev/sda/data
	[ -e $d ] || { echo "no $d"; ls /dev; return 1; }
	! grep -q 'sda$' /proc/usb || { echo "sda is a USB stick"; cat /proc/usb; return 1; }
	dd if=/dev/zero of=/tmp/scsi.in bs=512 count=1 2> /dev/null || return 1
	printf 'scsi-disk-%s' "$$" | dd of=/tmp/scsi.in conv=notrunc 2> /dev/null || return 1
	dd if=/tmp/scsi.in of=$d bs=512 seek=3 count=1 conv=notrunc 2> /dev/null || { echo "write failed"; return 1; }
	dd if=$d of=/tmp/scsi.out bs=512 skip=3 count=1 2> /dev/null || { echo "read failed"; return 1; }
	cmp /tmp/scsi.in /tmp/scsi.out || return 1
	[ "$(dd if=$d bs=512 skip=2047 count=1 2> /dev/null | wc -c)" -eq 512 ] || { echo "last sector"; return 1; }
	[ "$(dd if=$d bs=512 skip=2048 count=1 2> /dev/null | wc -c)" -eq 0 ] || { echo "past the end"; return 1; }
}
t -t blk scsi_disk scsi_disk
# The SD card (modules/sdmmc): the launcher gives every virt boot QEMU's
# SDHCI controller on PCI with a 4 GiB card, a high-capacity one (addressed
# by block). It reads and writes one block and several (more than one
# command's worth), past 2 GiB too, and ends where its CSD says. The Pi 3's
# card is its boot disk, on the sdhost and of standard capacity (addressed
# by byte): init mounted its ESP on /boot, which has the loader U-Boot
# started, and it ends where its CSD says (it is not written to).
sdmmc() {
	d=/dev/mmcblk0
	[ -e $d/data ] || { ls /dev; return 1; }
	if grep -q ' sdhost ' $d/info; then
		grep -q "^$d/p2 /boot fat " /proc/mounts && [ -s /boot/EFI/BOOT/BOOTAA64.EFI ] ||
			{ cat $d/info /proc/partitions /proc/mounts; return 1; }
		read -r _ size _ < $d/info
		n=$((size / 512))
		[ "$(dd if=$d/data bs=512 skip=$((n - 1)) count=1 2> /dev/null | wc -c)" -eq 512 ] || { echo "last block"; return 1; }
		[ "$(dd if=$d/data bs=512 skip=$n count=1 2> /dev/null | wc -c)" -eq 0 ] || { echo "past the end"; return 1; }
		return 0
	fi
	grep -Eq '^mmcblk0 4294967296 sdhci pci [0-9a-f]{2}:[0-9a-f]{2}\.[0-7] model "QEMU!" serial "[0-9a-f]{8}"$' $d/info ||
		{ cat $d/info; return 1; }
	seq 40000 > /tmp/sd.seq && dd if=/tmp/sd.seq of=/tmp/sd.in bs=512 count=300 2> /dev/null || return 1
	for at in 1:1 4000:300 5242880:300; do
		lba=${at%:*} n=${at#*:}
		dd if=/tmp/sd.in of=$d/data bs=512 seek=$lba count=$n conv=notrunc 2> /dev/null || { echo "write $at"; return 1; }
		dd if=$d/data of=/tmp/sd.out bs=512 skip=$lba count=$n 2> /dev/null || { echo "read $at"; return 1; }
		dd if=/tmp/sd.in of=/tmp/sd.want bs=512 count=$n 2> /dev/null && cmp /tmp/sd.want /tmp/sd.out || { echo "at $at"; return 1; }
	done
	[ "$(dd if=$d/data bs=512 skip=8388607 count=1 2> /dev/null | wc -c)" -eq 512 ] || { echo "last block"; return 1; }
	[ "$(dd if=$d/data bs=512 skip=8388608 count=1 2> /dev/null | wc -c)" -eq 0 ] || { echo "past the end"; return 1; }
}
t -t blk,sdmmc,smoke sdmmc sdmmc
# The Raspberry Pi's firmware (modules/rpi_firmware): /proc/firmware has
# what it answers through the mailbox: the revisions, the core clock (the
# sdhost's), the USB controller's power. A board without the mailbox has
# no /proc/firmware, and nothing to check.
rpi_firmware() {
	[ -e /proc/firmware ] || return 0
	cat /proc/firmware
	grep -q '^firmware 0x' /proc/firmware && grep -Eq '^clock core [1-9][0-9]* Hz$' /proc/firmware &&
		grep -Eq '^power usb (on|off)$' /proc/firmware
}
t -t smoke,rpi_firmware rpi_firmware rpi_firmware
# A full filesystem: the write that fills it gives what fit, the next says
# ENOSPC, natively and under the Linux layer (apk on a full /data stopped
# with "I/O error", the generic failure's errno), as does a pwrite into a
# grown file's hole (fileio_smoke.c, linux-smoke.c). An ext2 on the scratch
# disk's 64 MiB p3, before the partition test formats it (and leaves it the
# FAT volume the fstab test mounts).
enospc() {
	d=/dev/nvme1n1/p3
	mkdir -p /tmp/p3 && mkfs.ext2 $d > /dev/null && mount $d /tmp/p3 ext2 || return 1
	r=0
	/bin/etc/fileio_smoke enospc /tmp/p3 || r=1
	if grep -q "^linux$" /proc/modules && [ -x /bin/linux/linux-smoke ]; then
		linux /bin/linux/linux-smoke enospc /tmp/p3/lx || r=1
	fi
	rm -f /tmp/p3/fill /tmp/p3/lx
	umount /tmp/p3 || r=1
	return $r
}
t -t blk,fs,ext2,linux enospc enospc
t -t blk,smoke partitions partitions
t -t blk boot_disk boot_disk
t -t smoke boot_rc boot_rc
t -t fs,smoke fstab_mount fstab_mount

# A terminal is a directory (docs/tty.md): `data` is the terminal, `ctl` its
# state as text. /proc/self/fd/N names what an fd is open on, /proc/self/tty
# the controlling terminal's directory; a field written to ctl reads back,
# a bad line is refused.
tty_ctl() {
	tty=$(readlink /proc/self/tty) || return 1
	echo "tty: $tty"
	[ "$tty" = /dev/ttyS0 ] || return 1
	exec 5< /dev/console/data || return 1
	fd5=$(readlink /proc/self/fd/5)
	exec 5<&-
	echo "fd 5: $fd5"
	[ "$fd5" = /dev/console/data ] || return 1
	cat $tty/ctl
	old=$(grep '^cflag ' $tty/ctl)
	echo 'cflag 0x0' > $tty/ctl || return 1
	[ "$(grep '^cflag ' $tty/ctl)" = 'cflag 0x0' ] || return 1
	echo "$old" > $tty/ctl || return 1
	[ "$(grep '^cflag ' $tty/ctl)" = "$old" ] || return 1
	echo bogus > $tty/ctl 2>/dev/null && return 1
	grep -q '^winsize [0-9]* [0-9]*$' $tty/ctl
}
t -t tty,console tty_ctl tty_ctl

# The kernel's terminals (docs/tty.md): ttyS0 on the serial port (this
# session's) and tty1…tty9 on the console's text screens, init's getty on
# the ones /etc/ttys names (4), tty1's screen shown. `screen N` on the
# console's ctl shows another (0 the console's own), a tty refuses it.
# Text written to a screen not shown is kept: shown, it is drawn whole.
ttys() {
	ls /dev | tr '\n' ' '
	echo
	for t in ttyS0 tty1 tty5 tty9; do
		[ "$(ls /dev/$t | tr '\n' ' ')" = "ctl data " ] || return 1
	done
	ps
	[ "$(ps | grep -c ' getty$')" = 4 ] || return 1
	grep -q '^screen 1$' /dev/console/ctl || return 1
	echo 'screen 2' > /dev/console/ctl || return 1
	grep -q '^screen 2$' /dev/console/ctl || return 1
	echo 'screen 10' > /dev/console/ctl 2>/dev/null && return 1
	echo 'screen 0' > /dev/tty2/ctl 2>/dev/null && return 1
	[ "$(grep '^winsize' /dev/tty9/ctl)" = "$(grep '^winsize' /dev/console/ctl)" ] || return 1
	printf 'tty9\n' > /dev/tty9/data || return 1
	[ -e /dev/fb/ctl ] || { echo 'screen 1' > /dev/console/ctl; return 0; }
	read -r w h depth chan pitch mode < /dev/fb/ctl
	echo 'screen 0' > /dev/console/ctl || return 1
	blank=$(console_cells '')
	echo 'screen 1' > /dev/console/ctl || return 1
	shown=$(top_cells)
	printf '\033[H\033[JQQ\033[10;1H' > /dev/console/data
	hidden=$(top_cells)
	echo 'screen 0' > /dev/console/ctl || return 1
	back=$(top_cells)
	printf '\033[H\033[J\n' > /dev/console/data
	echo 'screen 1' > /dev/console/ctl
	echo "screen 1: $shown; screen 0 written meanwhile: $hidden; screen 0 shown: $back; blank: $blank"
	[ "$hidden" = "$shown" ] || { echo "screen 0's text was drawn while screen 1 was shown"; return 1; }
	[ "$back" != "$blank" ] && [ "$back" != "$shown" ] || { echo "screen 0 was not drawn when shown"; return 1; }
}

# /proc/self/exe names the program a process runs: the real path of the
# file its exec loaded (a multicall tool's hardlink by its own name).
proc_exe() {
	exe=$(/bin/sbase/readlink /proc/self/exe)
	echo "exe: $exe"
	[ "$exe" = /bin/sbase/readlink ] || return 1
	ls /proc/self | grep -qx exe
}
t -t proc proc_exe proc_exe

# /proc/self/fd listed while a file is open (here fd 7): its entry's path
# takes the mount table's lock, which the listing held, and the system froze
# (`find /`, its directories open, under the bind of /proc that
# `linux --root` makes). Listed directly and through a bind, by ls and find.
proc_fd_listing() {
	exec 7< /etc/policy
	ls /proc/self/fd | grep -qx 7 || return 1
	find /proc/self/fd > /dev/null || return 1
	mkdir -p /tmp/proc-bind && mount /proc /tmp/proc-bind bind || return 1
	ls /tmp/proc-bind/self/fd | grep -qx 7 && find /tmp/proc-bind/self > /dev/null
	r=$?
	umount /tmp/proc-bind
	exec 7<&-
	return $r
}
t -t proc proc_fd_listing proc_fd_listing

# A file made anew for each read is read from one text per open file
# (issue #472): a bind made between two reads of one fd is not in the rest
# it reads (whose lines would otherwise shift), and a new open shows it. A
# first read past its end (dd's skip seeks) reads nothing; umount takes
# the bind away.
proc_one_text() {
	[ -z "$(dd if=/proc/mounts bs=1 skip=1000000 2> /dev/null)" ] || return 1
	mkdir -p /tmp/one-text /tmp/one-text-at || return 1
	exec 4< /proc/mounts
	read -r first <&4
	mount /tmp/one-text /tmp/one-text-at bind || { exec 4<&-; return 1; }
	rest=$(cat <&4)
	exec 4<&-
	now=$(cat /proc/mounts)
	umount /tmp/one-text-at || return 1
	echo "$first"
	echo "$rest"
	! echo "$rest" | grep -q one-text-at && echo "$now" | grep -q '^/tmp/one-text /tmp/one-text-at bind ' \
		&& ! grep -q one-text-at /proc/mounts
}
t -t proc proc_one_text proc_one_text

# A pty comes from /dev/pts/clone: the fd it returns is /dev/pts/N/master,
# the pair's directory lists master, data and ctl, its window size is set
# through ctl, master is not openable by name, and the pair goes away with
# its last fd.
pty_clone() {
	exec 6<> /dev/pts/clone || return 1
	m=$(readlink /proc/self/fd/6)
	echo "master: $m"
	d=${m%/master}
	[ "$d" != "$m" ] || return 1
	[ "$(ls $d | sort | tr '\n' ' ')" = "ctl data master " ] || return 1
	grep -q '^winsize 24 80$' $d/ctl || return 1
	echo 'winsize 50 132' > $d/ctl || return 1
	grep -q '^winsize 50 132$' $d/ctl || return 1
	echo flush > $d/ctl || return 1
	( exec 7< $d/master ) 2>/dev/null && return 1
	exec 6>&-
	! [ -e $d/ctl ]
}
t -t tty pty_clone pty_clone

# Opening a directory's clone makes a new object and is its data
# (docs/events.md, docs/sockets-unix.md), as /dev/pts/clone is a new pair's
# master: the fd's link names it, its status is beside it, and it goes
# with its last fd, so none is left behind that nobody holds.
clone_open() {
	for c in /dev/event/clone /dev/timer/clone /dev/wait/clone /dev/sig/clone /net/unix/clone; do
		exec 7<> $c || { echo "open $c"; return 1; }
		d=$(readlink /proc/self/fd/7)
		echo "$c: $d"
		case $d in ${c%/clone}/*/data) ;; *) return 1 ;; esac
		d=${d%/data}
		[ -n "$(cat $d/status)" ] || return 1
		exec 7>&-
		! [ -e $d/status ] || { echo "$d outlived its fd"; return 1; }
	done
}
t -t events,netfs clone_open clone_open

# The console's keyboard map is a line of its ctl (docs/keymap.md): init
# loaded the Swiss one; `keymap PATH` loads another; a missing file is
# refused and leaves the loaded map; a pty has no keymap.
keymap_ctl() {
	grep -q '^keymap /lib/kbd/ch.map$' /dev/console/ctl || return 1
	echo 'keymap /lib/kbd/us.map' > /dev/console/ctl || return 1
	grep -q '^keymap /lib/kbd/us.map$' /dev/console/ctl || return 1
	echo 'keymap /lib/kbd/none.map' > /dev/console/ctl 2>/dev/null && return 1
	grep -q '^keymap /lib/kbd/us.map$' /dev/console/ctl || return 1
	exec 6<> /dev/pts/clone || return 1
	d=$(readlink /proc/self/fd/6)
	d=${d%/master}
	echo 'keymap /lib/kbd/us.map' > $d/ctl 2>/dev/null && return 1
	exec 6>&-
	echo 'keymap /lib/kbd/ch.map' > /dev/console/ctl
}
t -t console keymap keymap_ctl

# The board description (docs/pci-acpi-smp.md): one line per component,
# each ending in the source it came from. The CPUs and the interrupt
# controller are always there; the console UART and the PCIe host bridge
# too, except on a PC, whose arch does not need them described (QEMU's
# `pc` has no SPCR and no MCFG), and the host bridge on a board without
# PCI (the Raspberry Pi 3); an arm board with a GIC names its PSCI conduit
# (power-off, reboot); no component reads differently from the two
# sources. The devices the drivers attached for DMA have a line each
# (none on a board without such a device), all coherent on QEMU's boards,
# none with `dma=noncoherent` (aarch64).
platform() {
	cat /proc/platform
	grep -q '^source [a-z+]*$' /proc/platform || return 1
	grep -q '^cpus [0-9]* ([a-z]*)$' /proc/platform || return 1
	grep -q '^intc [a-z0-9]* 0x[0-9a-f]* ' /proc/platform || return 1
	if ! grep -q '^intc apic ' /proc/platform; then
		grep -q '^uart [a-z0-9]* 0x[0-9a-f]* ' /proc/platform || return 1
		# A board with PCI devices (not the Raspberry Pi 3) describes its
		# host bridge.
		if grep -qv '^#' /proc/pci; then
			grep -q '^pci ecam 0x[0-9a-f]* 0x[0-9a-f]* bus [0-9]*-[0-9]* (' /proc/platform || return 1
		fi
	fi
	if grep -q '^intc gic' /proc/platform; then
		grep -q '^psci [hs][vm]c ([a-z]*)$' /proc/platform || return 1
	fi
	! grep '^dma ' /proc/platform | grep -Evq '^dma (pci [0-9a-f:.]*|mmio 0x[0-9a-f]*)( 0x[0-9a-f]* 0x[0-9a-f]*( bus 0x[0-9a-f]*)?)* (non)?coherent$' || return 1
	if grep -q 'dma=noncoherent' /proc/cmdline; then
		! grep -q '^dma .* coherent$' /proc/platform || return 1
	elif ! grep -q '^model Raspberry Pi' /proc/platform; then
		# QEMU's boards mark their devices coherent; a Raspberry Pi's are
		# not (its tree says nothing, and the kernel takes that for not).
		! grep -q '^dma .* noncoherent$' /proc/platform || return 1
	fi
	! grep -q 'differs' /proc/platform
}
t -t smoke platform platform

# Interrupt domains (irqchip, docs/pci-acpi-smp.md "Interrupt domains"):
# the pl061 module makes the PL061 GPIO controller's lines interrupts of
# their own, behind the controller's one. Its probe sets an idle input to
# interrupt while low and enables that line's interrupt through the
# kernel; the handler turns it off. Both the controller's interrupt and
# the line's (a domain number, 4096 and up) were taken. Only a board
# without ACPI tables has the PL061 (QEMU's aarch64 GICv3 boots); on the
# others the module finds none.
irq_counts() {
	# The count of the handler called "$1" ("$2" its second word) at an
	# interrupt of at least $3.
	while read -r n count w1 w2; do
		[ "$w1" = "$1" ] && [ "$w2" = "$2" ] && [ "${n%:}" -ge "$3" ] && [ "$count" -gt 0 ] && return 0
	done < /proc/interrupts
	return 1
}
irq_domain() {
	insmod /lib/modules/pl061 && grep -q "^pl061$" /proc/modules || return 1
	grep -q '^source dt$' /proc/platform && grep -q '^intc gicv' /proc/platform || return 0
	i=0
	while [ $i -lt 5 ] && ! irq_counts pl061 check 4096; do
		sleep 1
		i=$((i + 1))
	done
	irq_counts pl061 check 4096 && irq_counts pl061 "" 32 || { cat /proc/interrupts; return 1; }
}
t -t irq,modules irq_domain irq_domain

# FAT read-write (the fat module, modules/fat/fatvol, host-tested against
# dosfstools): on the launcher's FAT16 disk, which /ok mounted at /tmp/fat,
# files and a directory with long names are written, moved across
# directories, a time set; the volume is unmounted and mounted again and
# all of it reads back, then goes. The host runs `fsck.fat -n` on the image
# after the boot.
fat_rw() {
	d=/tmp/fat
	dev=$(grep " $d fat " /proc/mounts | cut -d' ' -f1)
	[ -n "$dev" ] || { echo "$d is not mounted"; return 1; }
	mkdir "$d/A long directory" && echo one > "$d/A long directory/First File.txt" || return 1
	dd if=/bin/sbase/ls of=$d/big bs=1024 count=100 2>/dev/null && cp $d/big /tmp/fat-big || return 1
	mv "$d/A long directory/First File.txt" $d/moved.txt && mv "$d/A long directory" $d/dir2 || return 1
	mkdir $d/dir2/sub && mv $d/big $d/dir2/sub/big && echo two >> $d/moved.txt || return 1
	touch -T 981173106 $d/moved.txt || return 1
	umount $d && mount $dev $d fat || return 1
	ls $d
	[ "$(cat $d/moved.txt)" = "one
two" ] && cmp $d/dir2/sub/big /tmp/fat-big || return 1
	l=$(ls -l $d/moved.txt)
	echo "ls: $l"
	case $l in *"Feb 03  2001"*) ;; *) return 1 ;; esac
	rm -r $d/dir2 $d/moved.txt /tmp/fat-big && [ "$(ls $d)" = msg ]
}
t -t fat fat_rw fat_rw
# How full the FAT volume is (statvfs, the fat module's statfs): a file
# takes its clusters and gives them back (fileio_smoke.c).
fat_space() {
	/bin/etc/fileio_smoke space /tmp/fat
}
t -t fat fat_space fat_space

# USB (docs/usb.md): every boot has a USB controller (the virt boards'
# xHCI, the Raspberry Pi 3's DWC2) with a hub on its first port and a
# memory stick behind the hub, the same FAT volume as /dev/vda/data. The
# stick is enumerated on the USB thread after the modules load, so the
# test waits for its /dev/$USB_DISK/data: sdb where sda is the virtio-scsi
# disk (the kernel hands out the sd letters to both), sda on the Pi 3,
# which has no other; /proc/usb lists the hub and the stick with their
# drivers, the stick's line naming its disk; the volume mounts and reads.
export USB_DISK=sda
grep -q ' virtio_scsi ' /dev/sda/info 2>/dev/null && USB_DISK=sdb
wait_for() {
	n=$1
	shift
	while ! "$@" 2>/dev/null; do
		n=$((n - 1))
		[ $n -gt 0 ] || return 1
		sleep 1
	done
}
usb_disk() {
	wait_for 30 test -e /dev/$USB_DISK/data || { cat /proc/usb; return 1; }
	cat /proc/usb
	grep -q ' hub ' /proc/usb || return 1
	grep -q ":usb_storage $USB_DISK$" /proc/usb || return 1
	mkdir -p /tmp/usb && mount /dev/$USB_DISK/data /tmp/usb fat || return 1
	[ "$(cat /tmp/usb/msg)" = fat-msg ]
}
t -t usb,blk usb_disk usb_disk

# Every disk says what it is in /dev/<disk>/info: its name, its size and
# its driver's description (where it is, model, serial). The launcher's
# NVMe controllers have serials, the virtio-scsi disk is QEMU's 1 MiB
# scsi-hd, and the stick is the /proc/usb device usb_storage took.
disk_info() {
	for f in /dev/*/info; do
		d=${f#/dev/}
		d=${d%/info}
		read -r name size rest < $f
		[ "$name" = "$d" ] && [ "$size" -gt 0 ] || { echo "$f: $name $size $rest"; return 1; }
	done
	ls /dev/sda | grep -qx info || return 1
	grep -Eq '^nvme0n1 [0-9]+ nvme pci [0-9a-f]{2}:[0-9a-f]{2}\.[0-7] model "[^"]+" serial "myos"$' /dev/nvme0n1/info &&
		grep -q ' serial "myos-scratch"$' /dev/nvme1n1/info &&
		grep -Eq '^vda [0-9]+ virtio_blk (pci|mmio) ' /dev/vda/info &&
		grep -Eq '^sda 1048576 virtio_scsi (pci|mmio) [^ ]+ target 0:0 vendor "QEMU" model "QEMU HARDDISK"$' /dev/sda/info ||
		{ cat /dev/*/info; return 1; }
	id=$(grep ":usb_storage $USB_DISK$" /proc/usb | cut -d' ' -f1)
	grep -Eq "^$USB_DISK [0-9]+ usb_storage usb $id [0-9a-f]{4}:[0-9a-f]{4} " /dev/$USB_DISK/info || { cat /dev/$USB_DISK/info /proc/usb; return 1; }
}
t -t blk disk_info disk_info

# mount(2) takes any existing directory that is not a mount point yet, at
# the top level as below it, and umount gives the directory back; a busy
# mount stays, and a directory a mount hangs from is neither renamed nor
# removed (README, VFS). The stick of usb_disk is the disk.
mount_rules() {
	umount /tmp/usb && ! [ -e /tmp/usb/msg ] || return 1
	mount /dev/$USB_DISK/data /tmp/none fat 2>&1 | grep -q 'no such directory' || return 1
	mount /dev/$USB_DISK/data /mnt fat && [ "$(cat /mnt/msg)" = fat-msg ] || return 1
	mount /dev/$USB_DISK/data /tmp/usb fat 2>&1 | grep -q 'already mounted' || return 1
	umount /mnt && mkdir -p /tmp/a/b && mount /dev/$USB_DISK/data /tmp/a/b fat || return 1
	grep -q "^/dev/$USB_DISK/data /tmp/a/b fat " /proc/mounts || return 1
	mount /dev/$USB_DISK/data /tmp/a/b fat 2>&1 | grep -q 'already a mount point' || return 1
	! mv /tmp/a /tmp/c 2>/dev/null || return 1
	exec 4< /tmp/a/b/msg
	umount /tmp/a/b 2>&1 | grep -q busy || return 1
	exec 4<&-
	umount /tmp/a/b && rmdir /tmp/a/b /tmp/a || return 1
	umount /tmp 2>&1 | grep -q 'not a mount point of a disk'
}
t -t fs,sec mount_rules mount_rules

# Hot-plug: the host plugs a second stick into a root port through the
# QEMU monitor (user/tests/host.sh); it enumerates and reads; pulled out
# again, its /dev entry goes (nothing holds it).
usb_hotplug() {
	echo "HOST tests usb-plug" >&3
	wait_for 30 test -e /dev/sdc/data || { cat /proc/usb; return 1; }
	cat /proc/usb
	grep -q 'port 2 super .*:usb_storage sdc$' /proc/usb || return 1
	/bin/sbase/dd if=/dev/sdc/data of=/tmp/usb-sdc.bin bs=512 count=1 2>/dev/null || return 1
	[ "$(/bin/sbase/wc -c < /tmp/usb-sdc.bin)" -eq 512 ] || return 1
	echo "HOST tests usb-unplug" >&3
	wait_for 30 sh -c '! test -e /dev/sdc/data' || { cat /proc/usb; return 1; }
	cat /proc/usb
	! grep -q gone /proc/usb
}
t -t usb usb_hotplug usb_hotplug

# Unplug while busy, then reuse. A disk pulled out while a filesystem is
# mounted from it (or an fd holds it) cannot be unregistered, so it stays as a
# `gone` entry failing its I/O. Nothing calls back when the mount/fd finally
# goes, so unless usb_storage sweeps released `gone` disks, its /dev name and
# table slot leak: the next stick lands on sdd (then sde…) and repeated
# unplug-while-busy runs the table out. Here: mount the stick, pull it while
# mounted, unmount, re-plug — the fresh stick must reuse sdc (not leak to sdd),
# and the kernel must not fault across the busy unplug.
usb_hotplug_busy_reuse() {
	mkdir -p /tmp/usbm
	# Settle from the previous test's unplug before we drive our own.
	sleep 2
	echo "HOST tests usb-plug" >&3
	wait_for 40 sh -c 'grep -q "port 2 .*usb_storage sdc" /proc/usb' \
		|| { echo "reuse: no sdc to start"; cat /proc/usb; return 1; }
	# Mount it, then pull it while mounted: blk_unregister is refused, so the
	# disk must stay as a `gone` entry failing its I/O (and not fault).
	mount /dev/sdc/data /tmp/usbm fat || { echo "reuse: mount failed"; cat /proc/usb; return 1; }
	echo "HOST tests usb-unplug" >&3
	sleep 3
	# Let go: the mount was the only holder, so the disk is now reclaimable.
	umount /tmp/usbm || { echo "reuse: umount failed"; cat /proc/usb; return 1; }
	sleep 1
	# Re-plug. The freed slot and sdc name must be reused, so the fresh stick
	# enumerates on port 2 as sdc again and no leaked disk lingers on sdd.
	echo "HOST tests usb-plug" >&3
	wait_for 40 sh -c 'grep -q "port 2 .*usb_storage sdc" /proc/usb' \
		|| { echo "reuse: replugged stick is not sdc — slot leaked"; cat /proc/usb; return 1; }
	test -e /dev/sdd/data && { echo "reuse: a leaked disk lingers on sdd"; cat /proc/usb; return 1; }
	/bin/sbase/dd if=/dev/sdc/data of=/dev/null bs=512 count=1 2> /dev/null || { echo "reuse: sdc will not read"; return 1; }
	echo "HOST tests usb-unplug" >&3
	wait_for 30 sh -c '! test -e /dev/sdc/data'
}
t -t usb usb_hotplug_busy_reuse usb_hotplug_busy_reuse

# The hub pulled out and pushed back. Its stick goes with it; the hub comes
# back into the same table entry, and its status-change endpoint must work
# there: the stick plugged in behind it afterwards is seen through the
# hub's report, not by the port scan of the hub's probe. (The pulled hub's
# status transfer must not stay in the entry's completion records and
# block the endpoint: issue #280.)
usb_hub_replug() {
	# usb_disk's mount, which mount_rules undoes when it runs: a stick in
	# use stays in /dev after it is pulled out.
	! grep -q ' /tmp/usb ' /proc/mounts || umount /tmp/usb || return 1
	echo "HOST tests hub-unplug" >&3
	# The stick goes first (a detach takes the children first), the hub after.
	wait_for 30 sh -c '! test -e /dev/$USB_DISK/data' || { cat /proc/usb; return 1; }
	wait_for 30 sh -c '! grep -q " hub " /proc/usb' || { cat /proc/usb; return 1; }
	echo "HOST tests hub-plug" >&3
	wait_for 30 grep -q ' hub ' /proc/usb || { cat /proc/usb; return 1; }
	sleep 2
	echo "HOST tests hub-disk" >&3
	wait_for 30 test -e /dev/$USB_DISK/data || { cat /proc/usb; return 1; }
	cat /proc/usb
	grep -q "port 1 .*:usb_storage $USB_DISK$" /proc/usb || return 1
	/bin/sbase/dd if=/dev/$USB_DISK/data of=/tmp/usb-sdb.bin bs=512 count=1 2>/dev/null || return 1
	[ "$(/bin/sbase/wc -c < /tmp/usb-sdb.bin)" -eq 512 ]
}
t -t usb usb_hub_replug usb_hub_replug

# Live frame count (allocated minus freed) from /proc/meminfo.
frames_live() {
	set -- $(grep '^FramesLive:' /proc/meminfo)
	echo "$2"
}

# Set `before` to the live frame count once it holds still for a second (at
# most a few): what the previous tests started may still be taking memory,
# like the xHCI driver's DMA pages for a USB unplug (never given back), and
# would count against the probe below.
frames_quiet() {
	n=0
	before=$(frames_live)
	while [ $n -lt 5 ]; do
		sleep 1
		now=$(frames_live)
		[ "$now" = "$before" ] && break
		before=$now
		n=$((n + 1))
	done
}

# Set `after` to the live frame count once the children that just exited
# have let go of their memory, as far as LIMIT frames above `before`: a
# parent reaps a child before the child, still in its exit on another CPU,
# has freed its address space (see mem_hog_survives), so the last one's
# frames may still be live. Sampled again for a few seconds while above the
# limit; a leak stays there. Run in this shell, not in `$(...)`, so the
# samples count the same processes as `before`.
frames_settled() {
	n=0
	after=$(frames_live)
	while [ "$((after - before))" -gt "$1" ] && [ $n -lt 5 ]; do
		sleep 1
		after=$(frames_live)
		n=$((n + 1))
	done
}

# Exiting a process must free all of its memory, page tables included
# (issue #284). Fork many children that exit at once (no exec, so the page
# cache does not grow) and confirm the live frame count returns to its
# baseline: a per-exit leak would ratchet it up.
mem_fork_no_leak() {
	frames_live > /dev/null # warm the grep path (caches its code once)
	frames_quiet
	i=0
	while [ $i -lt 60 ]; do
		(:)
		i=$((i + 1))
	done
	frames_settled 8
	echo "mem: fork/exit x60 FramesLive $before -> $after (delta $((after - before)))"
	[ "$((after - before))" -le 8 ]
}
t -t mm mem_fork_no_leak mem_fork_no_leak

# The same across fork+exec+exit of a real program. Its code is cached on the
# first run (PageCacheKiB), so after a warm-up the repeated runs must add no
# persistent frames.
mem_exec_no_leak() {
	/bin/etc/hello > /dev/null 2>&1 # warm: cache the program's pages
	frames_live > /dev/null
	frames_quiet
	i=0
	while [ $i -lt 40 ]; do
		/bin/etc/hello > /dev/null 2>&1
		i=$((i + 1))
	done
	frames_settled 16
	echo "mem: exec x40 FramesLive $before -> $after (delta $((after - before)))"
	[ "$((after - before))" -le 16 ]
}
t -t mm mem_exec_no_leak mem_exec_no_leak

# A userspace memory hog must never abort the kernel, and its pages must all
# come back. `memhog` mmaps and touches a large region in chunks; the kernel
# either serves it in full or, under real memory pressure, fails the fault so
# the hog dies (SIGSEGV). Either way the kernel stays up and the live frame
# count returns to its baseline.
#
# The sleep is load-bearing: a dying process is reaped by its parent (SIGCHLD)
# *before* it frees its address space — `reclaim_user_aspace` runs afterwards on
# the exiting task and the big mmap walk can be preempted, so for a moment after
# the shell returns the hog's pages are still live. Sample after it settles.
mem_hog_survives() {
	frames_live > /dev/null
	before=$(frames_live)
	/bin/etc/memhog 256 || true # may be SIGSEGV-killed under pressure
	sleep 1                     # let the exiting hog finish reclaiming
	after=$(frames_live)
	echo "mem: hog 256MiB FramesLive $before -> $after (delta $((after - before)))"
	[ -n "$after" ] && [ "$((after - before))" -le 64 ]
}
t -t mm mem_hog_survives mem_hog_survives

# /proc/<pid> (docs/proc.md): the shell running the tests is listed with its
# one thread, a child it starts names it as its parent once it has exec'd,
# and the USB host's thread is there as a kernel thread.
proc_pid() {
	ls /proc | grep -qx "$$" || { echo "no $$ in /proc"; return 1; }
	read pid name state ppid pgid sid threads size cpu child start kind < /proc/$$/status
	[ "$pid" = "$$" ] && [ "$threads" = 1 ] && [ "$kind" = user ] || { cat /proc/$$/status; return 1; }
	[ "$(ls /proc/$$/task)" = "$$" ] || { ls /proc/$$/task; return 1; }
	read tid tpid tname rest < /proc/$$/task/$$/status
	[ "$tid" = "$$" ] && [ "$tpid" = "$$" ] && [ "$tname" = "$name" ] || { cat /proc/$$/task/$$/status; return 1; }
	sleep 10 &
	kid=$!
	wait_for 10 grep -q "^$kid sleep blocked $$ " /proc/$kid/status
	rc=$?
	cat /proc/$kid/status
	kill $kid
	wait $kid
	# The USB bus's kernel thread, where xhci found a controller (it
	# publishes /proc/usb; the Raspberry Pi 3 has none).
	[ $rc -eq 0 ] && { [ ! -e /proc/usb ] || cat /proc/*/status | grep -q " usb .* kernel$"; }
}
t -t proc,smoke proc_pid proc_pid

# CPU time: the shell's own grows as it computes, its children's once it
# has waited for one that computed; the CPUs' idle time grows across a sleep.
proc_cpu_time() {
	read pid name state ppid pgid sid threads size cpu0 child0 rest < /proc/$$/status
	i=0
	while [ $i -lt 2000 ]; do i=$((i + 1)); done
	sh -c 'i=0; while [ $i -lt 2000 ]; do i=$((i + 1)); done'
	read pid name state ppid pgid sid threads size cpu1 child1 rest < /proc/$$/status
	echo "cpu $cpu0 -> $cpu1, children $child0 -> $child1"
	[ "$cpu1" -gt "$cpu0" ] && [ "$child1" -gt "$child0" ] || return 1
	idle0=0
	while read c what ms; do [ "$what" = idle ] && idle0=$((idle0 + ms)); done < /proc/cpu
	sleep 1
	idle1=0
	while read c what ms; do [ "$what" = idle ] && idle1=$((idle1 + ms)); done < /proc/cpu
	echo "idle $idle0 -> $idle1"
	[ "$idle1" -gt "$idle0" ]
}
t -t proc,sched proc_cpu_time proc_cpu_time

# Tickless idle (issue #367): a CPU other than 0 with nothing to run stops
# its 100 Hz tick while it halts, so its halts last far longer than the
# 10 ms a tick would allow. Across a 2 s sleep some CPU other than 0 is
# mostly idle and averages more than 20 ms a halt (one CPU: nothing to do).
idle_stats() {
	while read key sep n; do [ "$key" = idle_halts ] && echo $n; done < /proc/cpuinfo
	while read c what ms; do [ "$what" = idle ] && echo $ms; done < /proc/cpu
}
idle_tickless() {
	set -A s0 $(idle_stats)
	n=$((${#s0[*]} / 2))
	[ $n -gt 1 ] || return 0
	sleep 2
	set -A s1 $(idle_stats)
	echo "halts per CPU, then idle ms: ${s0[*]} -> ${s1[*]}"
	c=1
	while [ $c -lt $n ]; do
		halts=$((s1[c] - s0[c]))
		idle=$((s1[n + c] - s0[n + c]))
		[ $idle -ge 1000 ] && [ $((halts * 20)) -lt $idle ] && return 0
		c=$((c + 1))
	done
	return 1
}
t -t sched idle_tickless idle_tickless

# The console's input interrupts (issue #367): the UART's receive interrupt
# and the keyboard's (PS/2 IRQ 1 through the I/O APIC on x86_64, the
# virtio-input device's elsewhere) are in /proc/interrupts, so nothing
# polls them and CPU 0 idles without its tick too: across a 2 s sleep its
# halts average more than 20 ms (the cursor blink wakes it twice a second).
console_irqs() {
	grep -q ' uart$' /proc/interrupts \
		&& grep -Eq ' (ps2 keyboard|virtio-input)$' /proc/interrupts \
		|| { cat /proc/interrupts; return 1; }
}
t -t sched,console console_irqs console_irqs
idle_cpu0() {
	set -A s0 $(idle_stats)
	n=$((${#s0[*]} / 2))
	sleep 2
	set -A s1 $(idle_stats)
	halts=$((s1[0] - s0[0]))
	idle=$((s1[n] - s0[n]))
	echo "CPU 0: $halts halts, $idle ms idle"
	[ $idle -ge 1000 ] && [ $((halts * 20)) -lt $idle ]
}
t -t sched idle_cpu0 idle_cpu0

# A module's character device is a directory: the NIC's `data` is the
# device, its `ctl` names the MAC and whether its interrupt works.
net_ctl() {
	[ -d /dev/net0 ] || return 0
	[ -c /dev/net0/data ] || return 1
	[ "$(ls /dev/net0 | sort | tr '\n' ' ')" = "ctl data " ] || return 1
	grep -q '^mac [0-9a-f:]*$' /dev/net0/ctl || return 1
	grep -q '^irq o' /dev/net0/ctl
}
t -t netfs,virtio_net net_ctl net_ctl

# netd's DHCP lease in /net/ndb: QEMU's, whose DNS server the launcher moves
# to 10.0.2.4 (add_virtio_net); the full mode's lookups go there.
net_lease() {
	[ -d /dev/net0 ] || return 0
	i=0
	while [ ! -s /net/ndb ] && [ $i -lt 10 ]; do
		sleep 1
		i=$((i + 1))
	done
	grep -q '^ip=10\.0\.2\.15 ipmask=255\.255\.255\.0 ipgw=10\.0\.2\.2$' /net/ndb \
		&& grep -q '^	dns=10\.0\.2\.4$' /net/ndb \
		|| { cat /net/ndb; return 1; }
}
t -t netfs,virtio_net net_lease net_lease

# isatty through the libc (the /proc/self/fd link, no ioctl): the shell's
# stdin is the console, its captured stdout a file.
isatty_fds() {
	[ -t 0 ] && ! [ -t 1 ]
}
t -t tty isatty isatty_fds

# CR on the screen goes back to the line's start, as a shell redrawing its
# line needs (oksh on Up or Tab: CR, the prompt, the line). The top row's
# first two 8x8 cells, read back from the framebuffer with the cursor moved
# away (it blinks): "QQ" changes them, "QQ", CR and two spaces leaves them as
# on a cleared screen (with the CR dropped the spaces would land after the
# Qs).
# The console's screen (0) is shown for these checks, tty1's again after.
top_cells() {
	for y in 0 1 2 3 4 5 6 7; do
		dd if=/dev/fb/data bs=64 count=1 skip=$((y * pitch / 64)) 2> /dev/null
	done | cksum
}

console_cells() {
	printf '\033[H\033[J%b\033[10;1H' "$1" > /dev/console/data
	top_cells
}

console_cr() {
	read -r w h depth chan pitch mode < /dev/fb/ctl
	echo 'screen 0' > /dev/console/ctl || return 1
	blank=$(console_cells '')
	qq=$(console_cells 'QQ')
	cr=$(console_cells 'QQ\r  ')
	printf '\033[H\033[J\n' > /dev/console/data
	echo 'screen 1' > /dev/console/ctl
	echo "cleared: $blank; QQ: $qq; QQ, CR, spaces: $cr"
	[ "$qq" != "$blank" ] || { echo "QQ did not reach the screen"; return 1; }
	[ "$cr" = "$blank" ] || { echo "QQ, CR, spaces: the Qs are still there"; return 1; }
}
t -t console,tty console_cr console_cr

# Inserting and deleting lines moves the rows below the cursor (termcap `al`
# and `dl`: vim opening a line with Enter or `o`), as does reverse index at
# the top (`sr`): the first two cells of the top three rows read back must
# be as if the text were written already moved.
# Each pattern goes to the console in one write (printf to the console
# itself writes line by line), and the rows are read until two reads agree:
# now and then one capture came back without the pattern's last line on
# aarch64 (issue #470).
console_capture() {
	for y in $(seq 0 23); do
		dd if=/dev/fb/data bs=64 count=1 skip=$((y * pitch / 64)) 2> /dev/null
	done | cksum
}

console_rows() {
	printf '\033[H\033[J%b\033[10;1H' "$1" | dd of=/dev/console/data bs=4096 2> /dev/null
	last=$(console_capture)
	for i in 1 2 3 4 5; do
		now=$(console_capture)
		[ "$now" = "$last" ] && break
		last=$now
		sleep 1
	done
	echo "$now"
}

console_lines() {
	read -r w h depth chan pitch mode < /dev/fb/ctl
	echo 'screen 0' > /dev/console/ctl || return 1
	moved=$(console_rows '\r\nA\r\nB')
	il=$(console_rows 'A\r\nB\033[H\033[L')
	ri=$(console_rows 'A\r\nB\033[H\033M')
	dl=$(console_rows 'X\r\nA\r\nB\033[H\033[M')
	up=$(console_rows 'A\r\nB')
	printf '\033[H\033[J\n' > /dev/console/data
	echo 'screen 1' > /dev/console/ctl
	echo "moved: $moved; IL: $il; RI: $ri; DL: $dl; up: $up"
	[ "$il" = "$moved" ] && [ "$ri" = "$moved" ] && [ "$dl" = "$up" ]
}
t -t console,tty console_lines console_lines
t -t console,tty ttys ttys

# Security (docs/security.md). The tests run as root in the admin domain.
# A policy with three more users, alice and bob (passwords alice-pw and
# bob-pw), their homes under /tmp/sec/home and programs under /tmp/sec/bin
# that run untrusted, and carol (carol-pw), whose domain `app` may only
# append to /tmp, is loaded for the tests and put back after them.
SEC=/bin/etc/sec
sec_as() {
	user=$1
	shift
	$SEC as $user -p $user-pw "$@"
}
sec_setup() {
	[ "$($SEC ctx)" = "0 root admin" ] || return 1
	mkdir -p /tmp/sec/home/alice/.ssh /tmp/sec/home/bob /tmp/sec/bin || return 1
	cp /bin/sbase/cat /bin/sbase/cp /bin/etc/sec /tmp/sec/bin/ || return 1
	echo disk-a > /tmp/sec/disk-a && echo disk-b > /tmp/sec/disk-b || return 1
	cat /etc/policy > /tmp/sec/policy || return 1
	for u in alice bob; do
		h=$(printf '%s' "salt$u-pw" | /bin/sbase/sha256sum | cut -d' ' -f1)
		echo "user $u groups: dev domains: shell untrusted password: sha256:salt:$h" >> /tmp/sec/policy
	done
	h=$(printf '%s' "saltcarol-pw" | /bin/sbase/sha256sum | cut -d' ' -f1)
	echo "user carol domains: app login: app password: sha256:salt:$h" >> /tmp/sec/policy
	printf '%s\n' 'label /tmp/sec/home/$u/** home($u)' 'label /tmp/sec/home/$u/.ssh/** secret($u)' \
		'label /tmp/sec/bin/** sys.bin' 'exec /tmp/sec/bin/** -> untrusted' 'domain app:' \
		'    sys.file {read} sys.bin {read exec} sys.lib {read exec} dev.tty(self) {read write}' \
		'    dev.common {read write} proc {read} tmp {read append exec}' >> /tmp/sec/policy
	printf '#!/bin/custom/sh\n/bin/etc/sec ctx\n' > /tmp/sec/bin/ctx.sh
	# Only alice may write her secrets, not even root.
	$SEC load /tmp/sec/policy && echo note > /tmp/sec/home/bob/note \
		&& ! (echo x > /tmp/sec/home/alice/.ssh/key) 2> /dev/null \
		&& sec_as alice /bin/custom/sh -c 'echo secret > /tmp/sec/home/alice/.ssh/key'
}
# A user's processes run as them, in their login domain, once the password
# is right.
sec_users() {
	out=$(sec_as alice $SEC ctx)
	echo "alice: $out"
	[ "$out" = "2 alice shell" ] || return 1
	! $SEC as alice -p wrong $SEC ctx 2> /dev/null
}
# Each user's home is theirs: alice writes hers, bob can neither read nor
# list it nor write there, and her keys are hers too.
sec_homes() {
	sec_as alice /bin/custom/sh -c 'echo hi > /tmp/sec/home/alice/f && cat /tmp/sec/home/alice/.ssh/key' || return 1
	! sec_as bob /bin/sbase/cat /tmp/sec/home/alice/f 2> /dev/null || return 1
	! sec_as bob /bin/sbase/ls /tmp/sec/home/alice > /dev/null 2>&1 || return 1
	! sec_as bob /bin/custom/sh -c 'echo x > /tmp/sec/home/alice/g' 2> /dev/null || return 1
	! sec_as alice /bin/sbase/cat /tmp/sec/home/bob/note 2> /dev/null || return 1
	# stat shows what the caller may do, and the owner from the label.
	l=$(sec_as bob /bin/sbase/ls -l /tmp/sec/home/bob/note)
	echo "bob: $l"
	case $l in -rwx------*bob*) ;; *) return 1 ;; esac
	! sec_as alice /bin/sbase/test -w /etc/policy
}
# A program under /tmp/sec/bin runs untrusted (a script too, whatever its
# interpreter): it reads its user's home and writes nothing there, and its
# user's secrets are out of reach.
sec_untrusted() {
	out=$(sec_as alice /tmp/sec/bin/sec ctx)
	echo "untrusted: $out"
	[ "$out" = "2 alice untrusted" ] || return 1
	out=$(sec_as alice /tmp/sec/bin/ctx.sh)
	echo "script: $out"
	[ "$out" = "2 alice untrusted" ] || return 1
	[ "$(sec_as alice /tmp/sec/bin/cat /tmp/sec/home/alice/f)" = hi ] || return 1
	! sec_as alice /tmp/sec/bin/cp /tmp/sec/home/alice/f /tmp/sec/home/alice/h 2> /dev/null || return 1
	! sec_as alice /tmp/sec/bin/cat /tmp/sec/home/alice/.ssh/key 2> /dev/null
}
# A rename takes a directory's files along, to names the label rules may
# read differently: it needs `remove` on each and `create` where they go.
# Root may not move alice's home out of /tmp/sec/home (her keys would be
# `tmp`), alice may not move /tmp/sec/home itself (bob's files would be
# hers), nor a file into bob's home in place of his; alice moves her own
# directory, root moves bob's home to another user's name (its label stays
# a home) and back.
sec_rename() {
	mkdir -p /tmp/sec/home/alice/d && echo in-d > /tmp/sec/home/alice/d/f || return 1
	! /bin/sbase/mv /tmp/sec/home/alice /tmp/sec/stolen 2> /dev/null || { echo "ESCALATION: root moved alice's home"; return 1; }
	! sec_as alice /bin/sbase/mv /tmp/sec/home /tmp/sec/h2 2> /dev/null \
		|| { echo "ESCALATION: alice moved /tmp/sec/home"; return 1; }
	! sec_as alice /bin/custom/sh -c 'echo mine > /tmp/sec/home/alice/m && /bin/sbase/mv /tmp/sec/home/alice/m /tmp/sec/home/bob/note' 2> /dev/null \
		|| return 1
	[ "$(cat /tmp/sec/home/bob/note)" = note ] || return 1
	[ "$(sec_as alice /bin/sbase/cat /tmp/sec/home/alice/.ssh/key)" = secret ] && [ ! -e /tmp/sec/stolen ] || return 1
	sec_as alice /bin/sbase/mv /tmp/sec/home/alice/d /tmp/sec/home/alice/e \
		&& [ "$(sec_as alice /bin/sbase/cat /tmp/sec/home/alice/e/f)" = in-d ] || return 1
	/bin/sbase/mv /tmp/sec/home/bob /tmp/sec/home/bob2 && /bin/sbase/mv /tmp/sec/home/bob2 /tmp/sec/home/bob \
		&& [ "$(sec_as bob /bin/sbase/cat /tmp/sec/home/bob/note)" = note ]
}
# An `append` grant without `write` (carol on /tmp): the file opens with
# O_APPEND and the fd adds to the end only, whatever pwrite's offset, and
# does not truncate (fileio_smoke.c, `append`).
sec_append() {
	printf 'line1\n' > /tmp/sec/app.txt || return 1
	[ "$(sec_as carol $SEC ctx)" = "4 carol app" ] || return 1
	sec_as carol /bin/etc/fileio_smoke append /tmp/sec/app.txt || return 1
	[ "$(cat /tmp/sec/app.txt)" = "line1
PW" ]
}
# `login: none`: system is nobody's to enter, not even from a domain with
# kernel.users write (admin); root, with no password, is.
sec_nologin() {
	! $SEC as system $SEC ctx 2> /dev/null || { echo "ESCALATION: system entered"; return 1; }
	[ "$($SEC as root $SEC ctx)" = "0 root admin" ]
}
# A mount or bind changes the tree for every process: it needs `write` on
# kernel.mounts, which `tmp {all}` does not give. alice cannot bind her
# home over /tmp (where a later root write would land in her files, run as
# packages), and the bind does not take: /tmp/sec is still there after.
# (A bind cannot be removed until reboot, so the test makes none.)
sec_mount() {
	! sec_as alice /bin/custom/mount /tmp/sec/home/alice /tmp bind 2> /dev/null \
		|| { echo "ESCALATION: alice bound over /tmp"; return 1; }
	[ -f /tmp/sec/policy ] || { echo "ESCALATION: alice's bind over /tmp took effect"; return 1; }
	! sec_as alice /bin/custom/umount /tmp 2> /dev/null || { echo "ESCALATION: alice unmounted /tmp"; return 1; }
	return 0
}
# A network conversation is its maker's: alice reads the status and data of
# her own TCP and unix conversations, bob cannot see them at all (the bytes
# of another user's connection are not his to read). The console keyboard
# is the administrator's (`dev`), not every terminal user's.
# Tagged netfs too, so a netd change selects it without the other sec
# tests: it loads the policy of the sec tests itself then, and puts the
# default back after.
sec_net() {
	own=
	[ -f /tmp/sec/policy ] || { sec_setup || return 1; own=1; }
	sec_net_checks
	rc=$?
	[ -z "$own" ] || sec_restore
	return $rc
}
sec_net_checks() {
	# A conversation lives while an fd holds it: a shell of alice's opens
	# one of each, names them in /tmp/sec-net, and holds them until that
	# file goes.
	rm -f /tmp/sec-net /tmp/sec-net.new
	sec_as alice /bin/custom/sh -c 'exec 3<> /net/tcp/clone 4<> /net/unix/clone || exit 1
		{ /bin/sbase/readlink /proc/self/fd/3; /bin/sbase/readlink /proc/self/fd/4; } > /tmp/sec-net.new
		/bin/sbase/mv /tmp/sec-net.new /tmp/sec-net
		while [ -e /tmp/sec-net ]; do /bin/sbase/sleep 1; done' &
	holder=$!
	i=0
	while [ ! -s /tmp/sec-net ] && [ $i -lt 30 ]; do
		sleep 1
		i=$((i + 1))
	done
	sec_net_peek
	rc=$?
	rm -f /tmp/sec-net
	wait $holder
	return $rc
}
sec_net_peek() {
	t=$(sed -n 1p /tmp/sec-net)
	u=$(sed -n 2p /tmp/sec-net)
	echo "alice's: $t $u"
	t=${t%/data}
	u=${u%/data}
	case $t in /net/tcp/*) ;; *) return 1 ;; esac
	case $u in /net/unix/*) ;; *) return 1 ;; esac
	sec_as alice /bin/sbase/cat $t/status > /dev/null || return 1
	! sec_as bob /bin/sbase/cat $t/status 2> /dev/null || { echo "LEAK: bob read alice's $t"; return 1; }
	! sec_as bob /bin/sbase/cat $t/data 2> /dev/null || { echo "LEAK: bob read alice's $t data"; return 1; }
	[ "$(sec_as alice /bin/sbase/cat $u/status)" = open ] || return 1
	! sec_as bob /bin/sbase/cat $u/status 2> /dev/null || { echo "LEAK: bob read alice's $u"; return 1; }
	! sec_as bob /bin/sbase/cat $u/data 2> /dev/null || { echo "LEAK: bob read alice's $u data"; return 1; }
	! sec_as alice /bin/sbase/cat /dev/console/kbd > /dev/null 2>&1 || { echo "LEAK: alice opened the keyboard"; return 1; }
	return 0
}
# A terminal is its session's user's (issue #442): alice opens neither
# this session's (root's, ttyS0) nor a getty's (system's, tty1) nor one no
# session holds (tty9), nor the console, and `ls -l` names a terminal's
# user. A pty she makes is hers, not bob's, until a session that enters
# bob claims it. A kernel terminal hangs up when it changes hands: what
# alice leaves running on tty8 after her session ends no longer reaches
# it (pty_smoke.c).
sec_tty() {
	for f in /dev/ttyS0/data /dev/ttyS0/ctl /dev/tty1/data /dev/tty1/ctl /dev/tty9/data /dev/console/data; do
		! sec_as alice /bin/custom/sh -c ": <> $f" 2> /dev/null || { echo "LEAK: alice opened $f"; return 1; }
	done
	l=$(/bin/sbase/ls -l /dev/tty1/data)
	echo "tty1: $l"
	case $l in *system*) ;; *) return 1 ;; esac
	sec_as alice /bin/etc/pty_smoke 7 bob bob-pw || return 1
	/bin/etc/pty_smoke 8 /dev/tty8/data alice alice-pw
}
# Signals: a user's processes, not another user's (init is system's).
sec_signal() {
	sec_as alice /bin/custom/sh -c '/bin/sbase/kill -0 $$' || return 1
	! sec_as alice /bin/sbase/kill -0 1 2> /dev/null
}
# Taking the system down needs `write` on kernel.power (docs/power.md):
# alice is refused (the program's status 1), whatever the name it runs
# under.
sec_power() {
	for cmd in poweroff reboot halt; do
		sec_as alice /bin/custom/$cmd
		status=$?
		echo "$cmd: $status"
		[ $status -eq 1 ] || return 1
	done
}
# Namespaces: a program sees only what it is given. Given disk-a (read
# only) as /tmp/sec/a, it reads that and not disk-b, and writes nothing; /
# lists only what is bound. An open fd it is handed is a deliberate grant.
# With the whole tree bound, a file bound into a directory of it is listed
# with the directory's own files (run-myos's apps, docs/packages.md).
sec_ns() {
	ns="$SEC ns /bin:read,exec /tmp/sec/a=/tmp/sec/disk-a:read --"
	[ "$($ns /bin/sbase/cat /tmp/sec/a)" = disk-a ] || return 1
	! $ns /bin/sbase/cat /tmp/sec/disk-b 2> /dev/null || return 1
	! $ns /bin/custom/sh -c 'echo x > /tmp/sec/a' 2> /dev/null || return 1
	out=$($ns /bin/sbase/ls / | tr '\n' ' ')
	echo "ls /: $out"
	[ "$out" = "bin tmp " ] || return 1
	[ "$($ns /bin/sbase/cat < /tmp/sec/disk-b)" = disk-b ] || return 1
	out=$($SEC ns / /bin/sbase/nsbound=/tmp/sec/disk-a -- /bin/sbase/ls /bin/sbase)
	echo "$out" | grep -qx nsbound && echo "$out" | grep -qx cat || { echo "ls /bin/sbase: $out"; return 1; }
	[ "$($SEC ns / /bin/sbase/nsbound=/tmp/sec/disk-a -- /bin/sbase/cat /bin/sbase/nsbound)" = disk-a ]
}
# A directory fd is a capability: a program handed one its namespace
# cannot name works beneath it with the rights it was opened with (all of
# them, or read only), and never above it (user/c/at_smoke.c).
sec_cap() {
	mkdir -p /tmp/sec/cap/sub && echo in > /tmp/sec/cap/f || return 1
	$SEC ns /bin:read,exec -- /bin/etc/at_smoke cap 3< /tmp/sec/cap || return 1
	$SEC ns /bin:read,exec /tmp/sec/cap:read -- /bin/custom/sh -c \
		"$SEC ns /bin:read,exec -- /bin/etc/at_smoke capro 3< /tmp/sec/cap"
}
# A Linux program is held to the same policy (the layer's file calls): it
# sees alice's file as her, not bob's. Tagged linux too, so a Linux layer
# change selects it without the other sec tests: it loads their policy
# and writes alice's file itself then, and puts the default back after,
# as sec_net does.
sec_linux() {
	own=
	[ -f /tmp/sec/policy ] || {
		sec_setup && sec_as alice /bin/custom/sh -c 'echo hi > /tmp/sec/home/alice/f' || return 1
		own=1
	}
	rc=0
	sec_as alice linux /bin/linux/linux-smoke mtime /tmp/sec/home/alice/f || rc=1
	! sec_as alice linux /bin/linux/linux-smoke mtime /tmp/sec/home/bob/note || rc=1
	[ -z "$own" ] || sec_restore
	return $rc
}
# The policy as it was: the users' processes are gone, alice and bob too.
sec_restore() {
	$SEC load /etc/policy && [ "$($SEC ctx)" = "0 root admin" ] && /bin/sbase/rm -r /tmp/sec
}
# alice knows only her own password; she must not become another user (bob
# needs his password; root is passwordless but only a domain with
# kernel.users write may enter it; system has `login: none`), nor load a
# policy.
sec_escalate() {
	for spec in "bob -p wrong" "bob -p alice-pw" "root" "system"; do
		out=$(sec_as alice $SEC as $spec $SEC ctx 2> /dev/null)
		echo "alice -> $spec: ${out:-refused}"
		case $out in
		""|*alice*) ;;
		*) echo "ESCALATION: alice became [$out]"; return 1 ;;
		esac
	done
	if sec_as alice $SEC load /tmp/sec/policy 2> /dev/null; then
		echo "ESCALATION: alice loaded a policy"; return 1
	fi
	return 0
}
# A domain written `*(self) {read}` grants read on the caller's OWN files
# only, never another user's (regression: the `*` kind must not stand in for
# the owner match).
sec_wildcard() {
	cat /tmp/sec/policy > /tmp/sec/wpolicy || return 1
	h=$(printf '%s' "saltprobe-pw" | /bin/sbase/sha256sum | cut -d' ' -f1)
	printf '%s
' "user probe groups: dev domains: probe login: probe password: sha256:salt:$h" 		'domain probe:' '    *(self) {read} sys.bin {read exec} sys.lib {read exec} dev.tty(self) {read write} dev.common {read write} proc {read}' 		>> /tmp/sec/wpolicy || return 1
	$SEC load /tmp/sec/wpolicy || return 1
	mkdir -p /tmp/sec/home/probe && echo pf > /tmp/sec/home/probe/pf || return 1
	# Own file: allowed. Alice's file: must be denied.
	[ "$($SEC as probe -p probe-pw /bin/sbase/cat /tmp/sec/home/probe/pf 2>/dev/null)" = pf ] || return 1
	if $SEC as probe -p probe-pw /bin/sbase/cat /tmp/sec/home/alice/f 2>/dev/null | grep -q hi; then
		echo "ESCALATION: probe (*(self)) read alice's file"; $SEC load /tmp/sec/policy; return 1
	fi
	$SEC load /tmp/sec/policy
}
t -t sec sec_setup sec_setup
t -t sec sec_users sec_users
t -t sec sec_homes sec_homes
t -t sec sec_untrusted sec_untrusted
t -t sec sec_rename sec_rename
t -t sec sec_append sec_append
t -t sec sec_nologin sec_nologin
t -t sec sec_mount sec_mount
t -t sec,netfs sec_net sec_net
t -t sec sec_tty sec_tty
t -t sec sec_signal sec_signal
t -t sec sec_escalate sec_escalate
t -t sec sec_wildcard sec_wildcard
t -t sec sec_power sec_power
t -t sec sec_ns sec_ns
t -t sec sec_cap sec_cap
if grep -q "^linux$" /proc/modules && [ -x /bin/linux/linux-smoke ]; then
	t -t sec,linux sec_linux sec_linux
fi
t -t sec sec_restore sec_restore

linux_loaded() {
	grep -q "^linux$" /proc/modules
}
lx_smoke() {
	capture $OUT/linux-smoke.log linux /bin/linux/linux-smoke
	cat $OUT/linux-smoke.log
	contains "LINUX-SMOKE OK" $OUT/linux-smoke.log
}
lx_dyn() {
	capture $OUT/linux-dyn.log linux /bin/linux/linux-dyn
	cat $OUT/linux-dyn.log
	contains "LINUX-DYN OK" $OUT/linux-dyn.log
}
# AF_UNIX between the layers (docs/sockets-unix.md): a Linux client
# connects to a native server by the name it listens under in /net/unix,
# as an Alpine X client reaches the X server's /tmp/.X11-unix/X0, and gets
# its bytes echoed.
lx_unix() {
	/bin/etc/unix_smoke echo /tmp/.lx-unix > $OUT/lx-unix-server.log 2>&1 &
	spid=$!
	capture $OUT/lx-unix.log linux /bin/linux/linux-smoke unix /tmp/.lx-unix
	# A client that never connected leaves the server in accept.
	contains "LINUX-UNIX OK" $OUT/lx-unix.log || kill $spid 2>/dev/null
	wait $spid
	status=$?
	cat $OUT/lx-unix.log $OUT/lx-unix-server.log
	[ $status = 0 ] && contains "LINUX-UNIX OK" $OUT/lx-unix.log
}
# The event fds from a chroot (how Alpine's programs run, docs/events.md):
# the launcher binds the system's /dev into the root, and
# /dev/wait/N/data opened there must still be a wait set, not a plain file.
lx_evfd_root() {
	mkdir -p /tmp/lxroot && cp /bin/linux/linux-smoke /tmp/lxroot/ || return 1
	capture $OUT/lx-evfd-root.log linux --root /tmp/lxroot /linux-smoke evfd
	cat $OUT/lx-evfd-root.log
	contains "LINUX-EVFD OK" $OUT/lx-evfd-root.log
}
# musl's resolver from a root (Alpine's programs) whose resolv.conf names
# a dead address first and the system's nameserver second: it asks both
# at once from one socket and takes the answer from whichever (the layer's
# recvfrom reports each datagram's sender; the smoke checks that too). The
# dead one goes first because QEMU's user network keeps one host socket
# per guest source port, with the last destination sent to as its peer,
# and turns a DNS answer's source back into the virtual nameserver only
# while that peer is it: listed second, the dead server made the answers
# arrive from the real server's address, which musl ignores.
lx_dns_root() {
	mkdir -p /tmp/lxroot/etc && cp /bin/linux/linux-smoke /tmp/lxroot/ || return 1
	ns=$(sed -n 's/^[[:space:]]*dns=//p' /net/ndb | head -1)
	[ -n "$ns" ] || return 1
	printf 'nameserver 192.0.2.1\nnameserver %s\noptions timeout:10 attempts:3\n' "$ns" > /tmp/lxroot/etc/resolv.conf
	capture $OUT/lx-dns-root.log linux --root /tmp/lxroot /linux-smoke dns "$ns" 192.0.2.1
	cat $OUT/lx-dns-root.log
	contains "LINUX-DNS OK" $OUT/lx-dns-root.log && return 0
	# What the smoke's probes do not say: the system's resolver, and musl
	# without the dead server, right after.
	cat /net/ndb
	dns example.com
	printf 'nameserver %s\n' "$ns" > /tmp/lxroot/etc/resolv.conf
	linux --root /tmp/lxroot /linux-smoke dns "$ns" 192.0.2.1
	return 1
}
# getcwd in a root under a bind (/mnt is /tmp/mnt bound over it): the
# kernel finds the cwd where it is stored, /tmp/mnt/..., and still names
# it in the root's terms (Python's venv and busybox's pwd loop on a
# getcwd that fails).
lx_cwd_bind() {
	mkdir -p /mnt/lxroot && cp /bin/linux/linux-smoke /mnt/lxroot/ || return 1
	capture $OUT/lx-cwd.log linux --root /mnt/lxroot /linux-smoke cwd
	cat $OUT/lx-cwd.log
	contains "LINUX-CWD OK" $OUT/lx-cwd.log
}
# The layer's trace switch (docs/linux-compat.md, Debugging): on, a call
# it does not implement still gets ENOSYS (and is reported on the console);
# at 2 the call is kept with its answer; off again; anything else refused.
lx_trace() {
	echo 1 > /proc/linux_trace && [ "$(cat /proc/linux_trace)" = 1 ] || return 1
	linux /bin/linux/linux-smoke enosys
	rc=$?
	[ $rc = 0 ] || return 1
	# Level 2 keeps each thread's last calls, published by the next write.
	echo 2 > /proc/linux_trace && linux /bin/linux/linux-smoke enosys || return 1
	echo 2 > /proc/linux_trace
	grep -q '^  4000 (0x1, 0x2, 0x3, 0x[0-9a-f]*) = -38$' /proc/linux_trace
	rc=$?
	echo 0 > /proc/linux_trace
	[ $rc = 0 ] && [ "$(cat /proc/linux_trace)" = 0 ] || return 1
	if echo maybe > /proc/linux_trace 2>/dev/null; then
		echo "/proc/linux_trace took 'maybe'"
		return 1
	fi
}
# The page cache (kernel/src/fs/pagecache.rs): the libraries one run maps
# serve the next (PageCacheKiB), and one rewritten in between is read as it
# is now. libsmoke2.so overwritten with libsmoke.so has no smoke2_name for
# linux-dyn's dlopen; put back, it has again.
lx_pagecache() {
	mkdir -p /tmp/pc && cp /lib/libsmoke.so /lib/libsmoke2.so /tmp/pc || return 1
	LD_LIBRARY_PATH=/tmp/pc linux /bin/linux/linux-dyn || return 1
	grep "^PageCacheKiB: [1-9]" /proc/meminfo || return 1
	cp /tmp/pc/libsmoke.so /tmp/pc/libsmoke2.so
	if LD_LIBRARY_PATH=/tmp/pc linux /bin/linux/linux-dyn; then
		echo "the rewritten libsmoke2.so was not seen"
		return 1
	fi
	cp /lib/libsmoke2.so /tmp/pc/libsmoke2.so
	LD_LIBRARY_PATH=/tmp/pc linux /bin/linux/linux-dyn && rm -r /tmp/pc
}
# A real Alpine package, downloaded at run time (jq + oniguruma + musl) by
# get-alpine (a package, installed by the full list like the others): it
# bootstraps apk in the default root, /data/alpine on the boot disk's data
# partition (mounted at /data), and apk installs jq there. Run chrooted in
# that root, jq counts the binds of /dev, /proc and /net into the root it
# sees in /proc/mounts (3, doubled).
lx_alpine() {
	grep -q ' /data ' /proc/mounts || { echo "no /data"; return 1; }
	run-myos get-alpine jq || return 1
	grep -q '^P:jq$' /data/alpine/lib/apk/db/installed || { ls -a /data/alpine; cat /data/alpine/etc/apk/world; return 1; }
	out=$(linux --root /data/alpine jq -Rrn '[inputs|select(test("/data/alpine/"))]|"ALPINE-JQ \(length*2)"' /proc/mounts)
	echo "$out"
	[ "$out" = "ALPINE-JQ 6" ]
}
# The Alpine disk the launcher prepares on the host (linux-compat/
# alpine-disks.sh: the roots of rustc, Python and an X client, from
# get-alpine -n, without their install scripts; /dev/nvme2n1/data, its
# writes dropped with the boot), mounted once at /tmp/alpine for the
# tests below: installing a toolchain in the emulated guest takes hours,
# Python ten minutes, mounting them seconds.
alpine_disk() {
	grep -q ' /tmp/alpine ' /proc/mounts && return 0
	mkdir -p /tmp/alpine && mount /dev/nvme2n1/data /tmp/alpine ext2 || return 1
	# get-alpine wrote the roots' resolv.conf on the host, for the host:
	# this system's resolver (the lease's dns= lines of /net/ndb, QEMU's
	# user network's when it names none), as it writes it in the guest.
	for r in /tmp/alpine/*/; do
		sed -n 's/^[[:space:]]*dns=/nameserver /p' /net/ndb > $r/etc/resolv.conf
		[ -s $r/etc/resolv.conf ] || echo "nameserver 10.0.2.3" > $r/etc/resolv.conf
	done
}
# Python (python3 and its 19 dependencies, ~45 MB) from the disk: the
# standard library, and the json and sqlite3 C extension modules.
lx_python() {
	alpine_disk || return 1
	out=$(linux --root /tmp/alpine/python python3 -c 'import json,sqlite3;print("PYTHON",json.loads("[42]")[0])')
	echo "$out"
	[ "$out" = "PYTHON 42" ]
}
# ... and its sockets: DNS (musl over UDP) and an HTTP GET (TCP).
lx_python_net() {
	alpine_disk || return 1
	out=$(linux --root /tmp/alpine/python python3 -c 'import urllib.request as u;print("HTTP",u.urlopen("http://example.com/").status)')
	echo "$out"
	[ "$out" = "HTTP 200" ]
}
# ... and its event loop, the end-to-end test of the event fds
# (docs/events.md): asyncio's selector is epoll (select.epoll over
# /dev/wait), its signal handler a wakeup fd in that selector, its
# subprocess a child watched from a thread; an echo server and client over
# an AF_UNIX socket (the layer's listeners are unix ones: issue #415), a
# timer, a timeout that elapses, SIGUSR1 handled by the loop, a child's
# output read back. The script runs inside the Python root (the launcher
# binds /dev, /proc and /net into it, not /tmp); it says what it is at on
# stderr and gives up after 60 s, and is killed after 120 s should the loop
# itself hang, so a step that never ends fails the test instead of
# stalling the boot.
lx_python_asyncio() {
	alpine_disk || return 1
	mkdir -p /tmp/alpine/python/tmp
	cat > /tmp/alpine/python/tmp/asyncio_smoke.py <<'PY'
import asyncio, os, selectors, signal, sys
sel = type(selectors.DefaultSelector()).__name__
def step(what):
    print('step:', what, file=sys.stderr, flush=True)
async def main():
    loop = asyncio.get_running_loop()
    async def handle(r, w):
        w.write((await r.read(100)).upper())
        await w.drain()
        w.close()
    step('unix server')
    srv = await asyncio.start_unix_server(handle, '/tmp/asyncio.sock')
    async with srv:
        step('connect')
        r, w = await asyncio.open_unix_connection('/tmp/asyncio.sock')
        w.write(b'asyncio')
        await w.drain()
        step('echo')
        echoed = await asyncio.wait_for(r.read(100), 10)
        w.close()
    step('sleep')
    t0 = loop.time()
    await asyncio.sleep(0.2)
    slept = loop.time() - t0 >= 0.15
    step('timeout')
    try:
        await asyncio.wait_for(asyncio.Event().wait(), 0.2)
        timed_out = False
    except asyncio.TimeoutError:
        timed_out = True
    step('signal')
    got = loop.create_future()
    loop.add_signal_handler(signal.SIGUSR1, got.set_result, 'USR1')
    loop.call_later(0.1, os.kill, os.getpid(), signal.SIGUSR1)
    sig = await asyncio.wait_for(got, 10)
    step('subprocess')
    p = await asyncio.create_subprocess_exec(sys.executable, '-c', 'print("sub")', stdout=asyncio.subprocess.PIPE)
    out, _ = await p.communicate()
    print('ASYNCIO', sel, echoed.decode(), slept, timed_out, sig, out.strip().decode(), p.returncode)
asyncio.run(asyncio.wait_for(main(), 60))
PY
	linux --root /tmp/alpine/python python3 /tmp/asyncio_smoke.py > $OUT/asyncio.out 2> $OUT/asyncio.err &
	apid=$!
	i=0
	while [ $i -lt 120 ] && kill -0 $apid 2>/dev/null; do
		sleep 1
		i=$((i + 1))
	done
	kill $apid 2>/dev/null
	wait $apid 2>/dev/null
	out=$(cat $OUT/asyncio.out)
	# The runner shows the last 40 lines: the first traceback (a thread's,
	# the cause) and the end of the last. Python's chained tracebacks end a
	# line in "exception:", which the host takes for a kernel crash marker
	# (src/boot_test.rs).
	{ head -n 30 $OUT/asyncio.err; echo ...; tail -n 4 $OUT/asyncio.err; } | sed 's/exception:/exception./'
	echo "$out"
	[ "$out" = "ASYNCIO EpollSelector ASYNCIO True True USR1 sub 0" ]
}
# An Alpine X client (xdpyinfo, from the disk) on the native X server (the
# tinyx package, installed as an app): it reaches Xfbdev's
# /tmp/.X11-unix/X0 over AF_UNIX and describes the display. The server
# takes the screen (/dev/fb/ctl says graphics) before it answers clients;
# xdpyinfo gives up at once while it does not answer yet, so it is run
# again, each run killed after 30 s.
lx_x11() {
	alpine_disk || return 1
	r=/tmp/alpine/x11
	run-myos tinyx:Xfbdev :0 -br > $OUT/lx-x11-server.log 2>&1 &
	xpid=$!
	i=0
	while [ $i -lt 60 ] && ! grep -q " graphics$" /dev/fb/ctl; do
		sleep 1
		i=$((i + 1))
	done
	ok=0
	try=0
	while [ $ok = 0 ] && [ $try -lt 5 ]; do
		DISPLAY=:0 linux --root $r xdpyinfo > $OUT/lx-x11.out 2>&1 &
		cpid=$!
		j=0
		while [ $j -lt 30 ] && kill -0 $cpid 2>/dev/null; do
			sleep 1
			j=$((j + 1))
		done
		kill $cpid 2>/dev/null
		wait $cpid 2>/dev/null
		grep -q "^name of display:" $OUT/lx-x11.out && ok=1
		try=$((try + 1))
	done
	kill $xpid 2>/dev/null
	i=0
	while [ $i -lt 10 ] && kill -0 $xpid 2>/dev/null; do
		sleep 1
		i=$((i + 1))
	done
	kill -9 $xpid 2>/dev/null
	wait $xpid 2>/dev/null
	[ $ok = 1 ] && return 0
	echo "after $try tries:"
	head -20 $OUT/lx-x11.out
	cat $OUT/lx-x11-server.log
	return 1
}
# Alpine's rustc (rust, LLVM and gcc: ~600 MB) from the disk: its
# libraries and allocator reservation need over 500 MiB of address space,
# of which `--version` touches some 30 MiB.
lx_rustc() {
	alpine_disk || return 1
	out=$(linux --root /tmp/alpine/rust rustc --version)
	echo "$out"
	case "$out" in "rustc 1."*) ;; *) return 1 ;; esac
}
# Without the feature: the boot script's default rc.local loaded the module
# with insmod (boot_rc); it shows up in /proc/modules, and a second load is
# refused.
lx_insmod() {
	linux_loaded && ! insmod /lib/modules/linux
}

if linux_loaded && [ -x /bin/linux/linux-smoke ]; then
	t -t linux linux_smoke lx_smoke
	t -t linux linux_dyn lx_dyn
	t -t linux,netfs linux_unix lx_unix
	t -t linux,events linux_evfd_root lx_evfd_root
	t -t linux,netfs linux_dns_root lx_dns_root
	t -t linux linux_cwd_bind lx_cwd_bind
	t -t linux linux_trace lx_trace
	t -t linux linux_pagecache lx_pagecache
	# The Alpine packages (get-alpine, an app: the packages and the network).
	t -t linux,net,pkg,heavy,alpine alpine_jq lx_alpine
	t -t linux,python,alpine-disk alpine_python lx_python
	t -t linux,python,alpine-disk,net,netfs alpine_python_net lx_python_net
	t -t linux,python,alpine-disk,events alpine_python_asyncio lx_python_asyncio
	t -t linux,x11,pkg,alpine-disk alpine_x11 lx_x11
	t -t linux,alpine-disk alpine_rustc lx_rustc
else
	t -t linux linux_insmod lx_insmod
fi
