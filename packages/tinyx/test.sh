# tinyx, installed as an app: startx runs Xfbdev and, once the server
# says it is ready, the client tinyx_smoke, which checks the screen's size,
# paints the framebuffer red through a window, puts a green MIT-SHM image
# on it and reads it back, and gets the Shift+A the host types through the
# QEMU monitor (tinyx_smoke.c); when the client exits, startx stops the
# server and the console has its screen back.
tinyx_run() {
	: > /tmp/tinyx.out
	run-myos tinyx:startx /bin/etc/tinyx_smoke >> /tmp/tinyx.out 2>&1 &
	spid=$!
	i=0
	while [ $i -lt 60 ] && ! grep -q -e ready -e "FAIL ]" -e "startx:" /tmp/tinyx.out 2>/dev/null; do
		sleep 1
		i=$((i + 1))
	done
	grep -q ready /tmp/tinyx.out && echo "HOST c-smokes sendkey shift-a" >&3
	# startx returns when the client is done and the server stopped.
	i=0
	while [ $i -lt 60 ] && grep -q " graphics$" /dev/fb/ctl; do
		sleep 1
		i=$((i + 1))
	done
	grep -q " graphics$" /dev/fb/ctl && kill $spid
	wait $spid
	rc=$?
	cat /tmp/tinyx.out
	[ $rc = 0 ] || { echo "startx: exit $rc"; return 1; }
	contains "[ OK ] tinyx shm" /tmp/tinyx.out || return 1
	contains "[ OK ] tinyx" /tmp/tinyx.out || return 1
	# The server's exit closed /dev/fb/ctl: the console has the screen back.
	grep -q " text$" /dev/fb/ctl
}
t tinyx tinyx_run
