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
t exec_limits exec_limits

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
t exec_script exec_script

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
t fd_offsets fd_offsets

# rmmod: a module that provides nothing (hello) unloads and loads again;
# one with a registration (the block driver's disks) is refused and keeps
# working.
rmmod_hello() {
	grep -q "^hello$" /proc/modules && rmmod hello && ! grep -q "^hello$" /proc/modules \
		&& insmod /lib/modules/hello && grep -q "^hello$" /proc/modules
}
rmmod_busy() {
	! rmmod virtio_blk && grep -q "^virtio_blk$" /proc/modules && ls /dev/vda
}
# A /proc/pci rescan re-probes the drivers: the disks are the same ones
# after it, and still readable.
pci_rescan() {
	ls /dev > /tmp/dev-before.txt
	echo rescan > /proc/pci || return 1
	ls /dev > /tmp/dev-after.txt
	cmp /tmp/dev-before.txt /tmp/dev-after.txt && /bin/sbase/tail -c 512 /dev/vda > /dev/null
}
t rmmod_hello rmmod_hello
t rmmod_busy rmmod_busy
t pci_rescan pci_rescan

# A terminal is a directory (docs/tty.md): `data` is the terminal, `ctl` its
# state as text. /proc/self/fd/N names what an fd is open on, /proc/self/tty
# the controlling terminal's directory; a field written to ctl reads back,
# a bad line is refused.
tty_ctl() {
	tty=$(readlink /proc/self/tty) || return 1
	echo "tty: $tty"
	[ "$tty" = /dev/console ] || return 1
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
t tty_ctl tty_ctl

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
t pty_clone pty_clone

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
t keymap keymap_ctl

# The board description (docs/pci-acpi-smp.md): one line per component,
# each ending in the source it came from. The CPUs and the interrupt
# controller are always there; the console UART and the PCIe host bridge
# too, except on a PC, whose arch does not need them described (QEMU's
# `pc` has no SPCR and no MCFG); no component reads differently from the
# two sources.
platform() {
	cat /proc/platform
	grep -q '^source [a-z+]*$' /proc/platform || return 1
	grep -q '^cpus [0-9]* ([a-z]*)$' /proc/platform || return 1
	grep -q '^intc [a-z0-9]* 0x[0-9a-f]* ' /proc/platform || return 1
	if ! grep -q '^intc apic ' /proc/platform; then
		grep -q '^uart [a-z0-9]* 0x[0-9a-f]* ' /proc/platform || return 1
		grep -q '^pci ecam 0x[0-9a-f]* 0x[0-9a-f]* bus [0-9]*-[0-9]* (' /proc/platform || return 1
	fi
	! grep -q 'differs' /proc/platform
}
t platform platform

# USB (docs/usb.md): every boot has an xHCI controller with a hub on its
# first port and a memory stick behind the hub, the same FAT volume as
# /dev/vda. The stick is enumerated on the USB thread after the modules
# load, so the test waits for /dev/sda; /proc/usb lists the hub and the
# stick with their drivers; the volume mounts and reads.
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
	wait_for 30 test -e /dev/sda || { cat /proc/usb; return 1; }
	cat /proc/usb
	grep -q ' hub ' /proc/usb || return 1
	grep -q ':usb_storage' /proc/usb || return 1
	mount /dev/sda /usb fat || return 1
	[ "$(cat /usb/msg)" = fat-msg ]
}
t usb_disk usb_disk

# Hot-plug: the host plugs a second stick into a root port through the
# QEMU monitor (user/tests/host.sh); it enumerates and reads; pulled out
# again, its /dev entry goes (nothing holds it).
usb_hotplug() {
	echo "HOST tests usb-plug" >&3
	wait_for 30 test -e /dev/sdb || { cat /proc/usb; return 1; }
	cat /proc/usb
	/bin/sbase/dd if=/dev/sdb of=/tmp/usb-sdb.bin bs=512 count=1 2>/dev/null || return 1
	[ "$(/bin/coreutils/wc -c < /tmp/usb-sdb.bin)" -eq 512 ] || return 1
	echo "HOST tests usb-unplug" >&3
	wait_for 30 sh -c '! test -e /dev/sdb' || { cat /proc/usb; return 1; }
	cat /proc/usb
	! grep -q gone /proc/usb
}
t usb_hotplug usb_hotplug

# A module's character device is a directory: the NIC's `data` is the
# device, its `ctl` names the MAC and whether its interrupt works.
net_ctl() {
	[ -d /dev/net0 ] || return 0
	[ -c /dev/net0/data ] || return 1
	[ "$(ls /dev/net0 | sort | tr '\n' ' ')" = "ctl data " ] || return 1
	grep -q '^mac [0-9a-f:]*$' /dev/net0/ctl || return 1
	grep -q '^irq o' /dev/net0/ctl
}
t net_ctl net_ctl

# isatty through the libc (the /proc/self/fd link, no ioctl): the shell's
# stdin is the console, its captured stdout a file.
isatty_fds() {
	[ -t 0 ] && ! [ -t 1 ]
}
t isatty isatty_fds

# CR on the screen goes back to the line's start, as a shell redrawing its
# line needs (oksh on Up or Tab: CR, the prompt, the line). The top row's
# first two 8x8 cells, read back from the framebuffer with the cursor moved
# away (it blinks): "QQ" changes them, "QQ", CR and two spaces leaves them as
# on a cleared screen (with the CR dropped the spaces would land after the
# Qs).
console_cells() {
	printf '\033[H\033[J%b\033[10;1H' "$1" > /dev/console/data
	for y in 0 1 2 3 4 5 6 7; do
		dd if=/dev/fb/data bs=64 count=1 skip=$((y * pitch / 64)) 2> /dev/null
	done | cksum
}

console_cr() {
	read -r w h depth chan pitch mode < /dev/fb/ctl
	blank=$(console_cells '')
	qq=$(console_cells 'QQ')
	cr=$(console_cells 'QQ\r  ')
	printf '\033[H\033[J\n' > /dev/console/data
	echo "cleared: $blank; QQ: $qq; QQ, CR, spaces: $cr"
	[ "$qq" != "$blank" ] || { echo "QQ did not reach the screen"; return 1; }
	[ "$cr" = "$blank" ] || { echo "QQ, CR, spaces: the Qs are still there"; return 1; }
}
t console_cr console_cr

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
# A real Alpine package, downloaded at run time (jq + oniguruma + musl), run
# chrooted in its Alpine root: it counts the binds of /dev, /proc and /net
# into that root it sees in /proc/mounts (3, doubled).
lx_alpine() {
	get-alpine jq || return 1
	out=$(linux --root /tmp/alpine jq -Rrn '[inputs|select(test("/tmp/alpine/"))]|"ALPINE-JQ \(length*2)"' /proc/mounts)
	echo "$out"
	[ "$out" = "ALPINE-JQ 6" ]
}
# Python (python3 and its 19 dependencies, ~45 MB in /tmp): the standard
# library, and the json and sqlite3 C extension modules.
lx_python() {
	get-alpine python3 || return 1
	out=$(linux --root /tmp/alpine python3 -c 'import json,sqlite3;print("PYTHON",json.loads("[42]")[0])')
	echo "$out"
	[ "$out" = "PYTHON 42" ]
}
# ... and its sockets: DNS (musl over UDP) and an HTTP GET (TCP).
lx_python_net() {
	out=$(linux --root /tmp/alpine python3 -c 'import urllib.request as u;print("HTTP",u.urlopen("http://example.com/").status)')
	echo "$out"
	[ "$out" = "HTTP 200" ]
}
# Alpine's rustc (rust, LLVM and gcc: ~600 MB) from the disk the launcher
# prepares on the host (linux-compat/alpine-disk.sh, /dev/nvme2n1): its
# libraries and allocator reservation need over 500 MiB of address space,
# of which `--version` touches some 30 MiB.
lx_rustc() {
	mount /dev/nvme2n1 /alpine-rust ext2 || return 1
	out=$(linux --root /alpine-rust/alpine rustc --version)
	echo "$out"
	case "$out" in "rustc 1."*) ;; *) return 1 ;; esac
}
# Without the feature: load the module now; it registers (the kernel prints
# `[ OK ] linux` on the console) and shows up in /proc/modules.
lx_insmod() {
	insmod /lib/modules/linux && linux_loaded
}

if linux_loaded && [ -x /bin/linux/linux-smoke ]; then
	t linux_smoke lx_smoke
	t linux_dyn lx_dyn
	if [ "$MODE" = full ]; then
		t alpine_jq lx_alpine
		t alpine_python lx_python
		t alpine_python_net lx_python_net
		t alpine_rustc lx_rustc
	fi
else
	t linux_insmod lx_insmod
fi
