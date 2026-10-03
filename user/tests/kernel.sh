# The kernel's own tests: what has no port directory of its own. The exec
# limits, and the Linux compatibility layer (docs/linux-compat.md): its
# module is in every image and loaded at boot when the image was built with
# the feature (`--features linux_compat`), with the musl test programs.

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
