# xev, installed as an app, on Xfbdev (the tinyx package), without a
# window manager: xev watches the root window's keyboard events (with no
# other window the keyboard's focus is the root), the host types "x"
# through the QEMU monitor and xev prints its KeyPress and KeyRelease;
# then its pointer events, the host moves and clicks the usb-mouse and
# xev prints a MotionNotify and the ButtonPress of button 1.
#
# A key typed before the server holds /dev/console/kbd goes to the console
# instead, so nothing is typed on a timer: the server takes the screen
# (/dev/fb/ctl says graphics) and then the keyboard and the pointer before
# it answers clients, and xev runs once run-myos has made its view and
# exec'd it (/proc/PID/status names it) and stays up connected. An event
# the server delivers before xev listens is lost, so it is sent again.

# Whether process $1 runs program $2 (run-myos execs it in the same pid).
runs() {
	[ "$(cut -d' ' -f2 /proc/$1/status 2>/dev/null)" = "$2" ]
}

# xev_events MASK REQUEST DONE PATTERN...: Xfbdev, and xev on the root
# with event mask MASK; the host's REQUEST (a `HOST c-smokes` line) is sent
# up to three times, until xev printed DONE; then every PATTERN must be in
# xev's output.
xev_events() {
	mask=$1 request=$2 done=$3
	shift 3
	run-myos tinyx:Xfbdev :0 -br > /tmp/xev-server.log 2>&1 &
	xpid=$!
	ok=0
	i=0
	while [ $i -lt 60 ] && ! grep -q " graphics$" /dev/fb/ctl; do
		sleep 1
		i=$((i + 1))
	done
	# xev gives up at once while the server does not answer yet.
	i=0
	while [ $i -lt 30 ]; do
		DISPLAY=:0 run-myos x11-apps -root -event "$mask" > /tmp/xev.out 2>&1 &
		epid=$!
		j=0
		while [ $j -lt 60 ] && kill -0 $epid 2>/dev/null && ! runs $epid xev; do
			sleep 1
			j=$((j + 1))
		done
		sleep 2
		runs $epid xev && break
		i=$((i + 1))
	done
	if runs $epid xev; then
		for try in 1 2 3; do
			echo "HOST c-smokes $request" >&3
			i=0
			while [ $i -lt 10 ] && ! grep -q "$done" /tmp/xev.out; do
				sleep 1
				i=$((i + 1))
			done
			grep -q "$done" /tmp/xev.out && break
		done
		ok=1
		for p in "$@"; do
			grep -q "$p" /tmp/xev.out || ok=0
		done
		kill $epid 2>/dev/null
		wait $epid 2>/dev/null
	fi
	kill $xpid 2>/dev/null
	wait $xpid 2>/dev/null
	[ $ok = 1 ] || cat /tmp/xev.out /tmp/xev-server.log
	[ $ok = 1 ]
}

xev_keys() {
	xev_events keyboard "sendkey x" KeyRelease "KeyPress event" "keysym 0x78, x"
}
t -t x11 xev xev_keys

# The pointer: /dev/console/mouse (usb_hid), host.sh's 10 right, 5 up and
# a left click.
xev_pointer() {
	xev_events mouse mouse ButtonRelease "MotionNotify event" "ButtonPress event" "button 1,"
}
t -t x11,mouse xev_pointer xev_pointer
