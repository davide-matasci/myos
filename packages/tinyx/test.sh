# tinyx, installed as a package: Xfbdev on :0, a client that checks the
# screen's size, paints the framebuffer red through a window and gets the
# Shift+A the host types through the QEMU monitor (tinyx_smoke.c); then the
# server's exit gives the console its screen back.
tinyx_run() {
	: > /tmp/tinyx.out
	Xfbdev :0 > /tmp/tinyx-server.log 2>&1 &
	xpid=$!
	/bin/etc/tinyx_smoke >> /tmp/tinyx.out 2>&1 &
	cpid=$!
	i=0
	while [ $i -lt 60 ] && ! grep -q -e ready -e "FAIL ]" /tmp/tinyx.out 2>/dev/null; do
		sleep 1
		i=$((i + 1))
	done
	grep -q ready /tmp/tinyx.out && echo "HOST c-smokes sendkey shift-a" >&3
	i=0
	while [ $i -lt 60 ] && ! grep -q -e "OK ] tinyx" -e "FAIL ]" /tmp/tinyx.out 2>/dev/null; do
		sleep 1
		i=$((i + 1))
	done
	kill $cpid $xpid 2>/dev/null
	wait $cpid $xpid 2>/dev/null
	cat /tmp/tinyx.out /tmp/tinyx-server.log
	contains "[ OK ] tinyx" /tmp/tinyx.out || return 1
	# The server's exit closed /dev/fb/ctl: the console has the screen back.
	grep -q " text$" /dev/fb/ctl
}
t tinyx tinyx_run
