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
	fi
else
	t linux_insmod lx_insmod
fi
