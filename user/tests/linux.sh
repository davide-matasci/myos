# The Linux compatibility layer (docs/linux-compat.md): the module is in
# every image and loaded at boot when the image was built with the
# feature (`--features linux_compat`), with the musl test programs.

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
	fi
else
	t linux_insmod lx_insmod
fi
