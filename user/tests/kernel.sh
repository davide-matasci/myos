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
t fd_identity fd_identity

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
# `pc` has no SPCR and no MCFG); an arm board names its PSCI conduit
# (power-off, reboot); no component reads differently from the two
# sources.
platform() {
	cat /proc/platform
	grep -q '^source [a-z+]*$' /proc/platform || return 1
	grep -q '^cpus [0-9]* ([a-z]*)$' /proc/platform || return 1
	grep -q '^intc [a-z0-9]* 0x[0-9a-f]* ' /proc/platform || return 1
	if ! grep -q '^intc apic ' /proc/platform; then
		grep -q '^uart [a-z0-9]* 0x[0-9a-f]* ' /proc/platform || return 1
		grep -q '^pci ecam 0x[0-9a-f]* 0x[0-9a-f]* bus [0-9]*-[0-9]* (' /proc/platform || return 1
	fi
	if grep -q '^intc gic' /proc/platform; then
		grep -q '^psci [hs][vm]c ([a-z]*)$' /proc/platform || return 1
	fi
	! grep -q 'differs' /proc/platform
}
t platform platform

# USB (docs/usb.md): every boot has an xHCI controller with a hub on its
# first port and a memory stick behind the hub, the same FAT volume as
# /dev/vda. The stick is enumerated on the USB thread after the modules
# load, so the test waits for /dev/sda; /proc/usb lists the hub and the
# stick with their drivers, the stick's line naming its disk; the volume
# mounts and reads.
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
	grep -q ':usb_storage sda$' /proc/usb || return 1
	mkdir -p /tmp/usb && mount /dev/sda /tmp/usb fat || return 1
	[ "$(cat /tmp/usb/msg)" = fat-msg ]
}
t usb_disk usb_disk

# mount(2) takes any existing directory that is not a mount point yet, at
# the top level as below it, and umount gives the directory back; a busy
# mount stays, and a directory a mount hangs from is neither renamed nor
# removed (README, VFS). The stick of usb_disk is the disk.
mount_rules() {
	umount /tmp/usb && ! [ -e /tmp/usb/msg ] || return 1
	mount /dev/sda /tmp/none fat 2>&1 | grep -q 'no such directory' || return 1
	mount /dev/sda /mnt fat && [ "$(cat /mnt/msg)" = fat-msg ] || return 1
	mount /dev/sda /tmp/usb fat 2>&1 | grep -q 'already mounted' || return 1
	umount /mnt && mkdir -p /tmp/a/b && mount /dev/sda /tmp/a/b fat || return 1
	grep -q '^/dev/sda /tmp/a/b fat ' /proc/mounts || return 1
	mount /dev/sda /tmp/a/b fat 2>&1 | grep -q 'already a mount point' || return 1
	! mv /tmp/a /tmp/c 2>/dev/null || return 1
	exec 4< /tmp/a/b/msg
	umount /tmp/a/b 2>&1 | grep -q busy || return 1
	exec 4<&-
	umount /tmp/a/b && rmdir /tmp/a/b /tmp/a || return 1
	umount /tmp 2>&1 | grep -q 'not a mount point of a disk'
}
t mount_rules mount_rules

# Hot-plug: the host plugs a second stick into a root port through the
# QEMU monitor (user/tests/host.sh); it enumerates and reads; pulled out
# again, its /dev entry goes (nothing holds it).
usb_hotplug() {
	echo "HOST tests usb-plug" >&3
	wait_for 30 test -e /dev/sdb || { cat /proc/usb; return 1; }
	cat /proc/usb
	grep -q 'port 2 super .*:usb_storage sdb$' /proc/usb || return 1
	/bin/sbase/dd if=/dev/sdb of=/tmp/usb-sdb.bin bs=512 count=1 2>/dev/null || return 1
	[ "$(/bin/coreutils/wc -c < /tmp/usb-sdb.bin)" -eq 512 ] || return 1
	echo "HOST tests usb-unplug" >&3
	wait_for 30 sh -c '! test -e /dev/sdb' || { cat /proc/usb; return 1; }
	cat /proc/usb
	! grep -q gone /proc/usb
}
t usb_hotplug usb_hotplug

# Unplug while busy, then reuse. A disk pulled out while a filesystem is
# mounted from it (or an fd holds it) cannot be unregistered, so it stays as a
# `gone` entry failing its I/O. Nothing calls back when the mount/fd finally
# goes, so unless usb_storage sweeps released `gone` disks, its /dev name and
# table slot leak: the next stick lands on sdc (then sdd…) and repeated
# unplug-while-busy runs the table out. Here: mount the stick, pull it while
# mounted, unmount, re-plug — the fresh stick must reuse sdb (not leak to sdc),
# and the kernel must not fault across the busy unplug.
usb_hotplug_busy_reuse() {
	mkdir -p /tmp/usbm
	# Settle from the previous test's unplug before we drive our own.
	sleep 2
	echo "HOST tests usb-plug" >&3
	wait_for 40 sh -c 'grep -q "port 2 .*usb_storage sdb" /proc/usb' \
		|| { echo "reuse: no sdb to start"; cat /proc/usb; return 1; }
	# Mount it, then pull it while mounted: blk_unregister is refused, so the
	# disk must stay as a `gone` entry failing its I/O (and not fault).
	mount /dev/sdb /tmp/usbm fat || { echo "reuse: mount failed"; cat /proc/usb; return 1; }
	echo "HOST tests usb-unplug" >&3
	sleep 3
	# Let go: the mount was the only holder, so the disk is now reclaimable.
	umount /tmp/usbm || { echo "reuse: umount failed"; cat /proc/usb; return 1; }
	sleep 1
	# Re-plug. The freed slot and sdb name must be reused, so the fresh stick
	# enumerates on port 2 as sdb again and no leaked disk lingers on sdc.
	echo "HOST tests usb-plug" >&3
	wait_for 40 sh -c 'grep -q "port 2 .*usb_storage sdb" /proc/usb' \
		|| { echo "reuse: replugged stick is not sdb — slot leaked"; cat /proc/usb; return 1; }
	test -e /dev/sdc && { echo "reuse: a leaked disk lingers on sdc"; cat /proc/usb; return 1; }
	/bin/sbase/dd if=/dev/sdb of=/dev/null bs=512 count=1 2> /dev/null || { echo "reuse: sdb will not read"; return 1; }
	echo "HOST tests usb-unplug" >&3
	wait_for 30 sh -c '! test -e /dev/sdb'
}
t usb_hotplug_busy_reuse usb_hotplug_busy_reuse

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
t mem_fork_no_leak mem_fork_no_leak

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
t mem_exec_no_leak mem_exec_no_leak

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
t mem_hog_survives mem_hog_survives

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
	[ $rc -eq 0 ] && cat /proc/*/status | grep -q " usb .* kernel$"
}
t proc_pid proc_pid

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
t proc_cpu_time proc_cpu_time

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
	# The runner turned the screen copy off (run.sh): on for the check.
	mirror=$(grep '^mirror ' /dev/console/ctl)
	echo 'mirror on' > /dev/console/ctl || return 1
	grep -q '^mirror on$' /dev/console/ctl || return 1
	blank=$(console_cells '')
	qq=$(console_cells 'QQ')
	cr=$(console_cells 'QQ\r  ')
	printf '\033[H\033[J\n' > /dev/console/data
	echo "$mirror" > /dev/console/ctl
	echo "cleared: $blank; QQ: $qq; QQ, CR, spaces: $cr"
	[ "$qq" != "$blank" ] || { echo "QQ did not reach the screen"; return 1; }
	[ "$cr" = "$blank" ] || { echo "QQ, CR, spaces: the Qs are still there"; return 1; }
}
t console_cr console_cr

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
		'    sys.file {read} sys.bin {read exec} sys.lib {read exec} dev.tty {read write}' \
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
sec_net() {
	n=$(sec_as alice /bin/sbase/cat /net/tcp/clone) || return 1
	sec_as alice /bin/sbase/cat /net/tcp/$n/status > /dev/null || return 1
	! sec_as bob /bin/sbase/cat /net/tcp/$n/status 2> /dev/null || { echo "LEAK: bob read alice's tcp $n"; return 1; }
	! sec_as bob /bin/sbase/cat /net/tcp/$n/data 2> /dev/null || { echo "LEAK: bob read alice's tcp $n data"; return 1; }
	u=$(sec_as alice /bin/sbase/cat /net/unix/clone) || return 1
	[ "$(sec_as alice /bin/sbase/cat /net/unix/$u/status)" = open ] || return 1
	! sec_as bob /bin/sbase/cat /net/unix/$u/status 2> /dev/null || { echo "LEAK: bob read alice's unix $u"; return 1; }
	! sec_as bob /bin/sbase/cat /net/unix/$u/data 2> /dev/null || { echo "LEAK: bob read alice's unix $u data"; return 1; }
	! sec_as alice /bin/sbase/cat /dev/console/kbd > /dev/null 2>&1 || { echo "LEAK: alice opened the keyboard"; return 1; }
	return 0
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
sec_ns() {
	ns="$SEC ns /bin:read,exec /tmp/sec/a=/tmp/sec/disk-a:read --"
	[ "$($ns /bin/sbase/cat /tmp/sec/a)" = disk-a ] || return 1
	! $ns /bin/sbase/cat /tmp/sec/disk-b 2> /dev/null || return 1
	! $ns /bin/custom/sh -c 'echo x > /tmp/sec/a' 2> /dev/null || return 1
	out=$($ns /bin/sbase/ls / | tr '\n' ' ')
	echo "ls /: $out"
	[ "$out" = "bin tmp " ] || return 1
	[ "$($ns /bin/sbase/cat < /tmp/sec/disk-b)" = disk-b ]
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
# sees alice's file as her, not bob's.
sec_linux() {
	sec_as alice linux /bin/linux/linux-smoke mtime /tmp/sec/home/alice/f || return 1
	! sec_as alice linux /bin/linux/linux-smoke mtime /tmp/sec/home/bob/note
}
# The policy as it was: the users' processes are gone, alice and bob too.
sec_restore() {
	$SEC load /etc/policy && [ "$($SEC ctx)" = "0 root admin" ] && /bin/coreutils/rm -r /tmp/sec
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
' "user probe groups: dev domains: probe login: probe password: sha256:salt:$h" 		'domain probe:' '    *(self) {read} sys.bin {read exec} sys.lib {read exec} dev.tty {read write} dev.common {read write} proc {read}' 		>> /tmp/sec/wpolicy || return 1
	$SEC load /tmp/sec/wpolicy || return 1
	mkdir -p /tmp/sec/home/probe && echo pf > /tmp/sec/home/probe/pf || return 1
	# Own file: allowed. Alice's file: must be denied.
	[ "$($SEC as probe -p probe-pw /bin/sbase/cat /tmp/sec/home/probe/pf 2>/dev/null)" = pf ] || return 1
	if $SEC as probe -p probe-pw /bin/sbase/cat /tmp/sec/home/alice/f 2>/dev/null | grep -q hi; then
		echo "ESCALATION: probe (*(self)) read alice's file"; $SEC load /tmp/sec/policy; return 1
	fi
	$SEC load /tmp/sec/policy
}
t sec_setup sec_setup
t sec_users sec_users
t sec_homes sec_homes
t sec_untrusted sec_untrusted
t sec_rename sec_rename
t sec_append sec_append
t sec_nologin sec_nologin
t sec_mount sec_mount
t sec_net sec_net
t sec_signal sec_signal
t sec_escalate sec_escalate
t sec_wildcard sec_wildcard
t sec_power sec_power
t sec_ns sec_ns
t sec_cap sec_cap
if grep -q "^linux$" /proc/modules && [ -x /bin/linux/linux-smoke ]; then
	t sec_linux sec_linux
fi
t sec_restore sec_restore

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
	mkdir -p /tmp/alpine-rust && mount /dev/nvme2n1 /tmp/alpine-rust ext2 || return 1
	out=$(linux --root /tmp/alpine-rust/alpine rustc --version)
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
	t linux_pagecache lx_pagecache
	if [ "$MODE" = full ]; then
		t alpine_jq lx_alpine
		t alpine_python lx_python
		t alpine_python_net lx_python_net
		t alpine_rustc lx_rustc
	fi
else
	t linux_insmod lx_insmod
fi
