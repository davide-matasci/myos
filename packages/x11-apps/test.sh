# xev, installed as an app, on Xfbdev (the tinyx package), without a
# window manager: xev watches the root window's keyboard events (with no
# other window the keyboard's focus is the root), the host types "x"
# through the QEMU monitor and xev prints its KeyPress and KeyRelease.
#
# A key typed before the server holds /dev/console/kbd goes to the console
# instead, so nothing is typed on a timer: the server takes the screen
# (/dev/fb/ctl says graphics) and then the keyboard before it answers
# clients, and xev runs once run-myos has made its view and exec'd it
# (/proc/PID/status names it) and stays up connected. A key the server
# delivers before xev listens is lost, so it is typed again.

# Whether process $1 runs program $2 (run-myos execs it in the same pid).
runs() {
	[ "$(cut -d' ' -f2 /proc/$1/status 2>/dev/null)" = "$2" ]
}

xev_keys() {
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
		DISPLAY=:0 run-myos x11-apps -root -event keyboard > /tmp/xev.out 2>&1 &
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
			echo "HOST c-smokes sendkey x" >&3
			i=0
			while [ $i -lt 10 ] && ! grep -q KeyRelease /tmp/xev.out; do
				sleep 1
				i=$((i + 1))
			done
			grep -q KeyRelease /tmp/xev.out && break
		done
		grep -q "KeyPress event" /tmp/xev.out && grep -q "keysym 0x78, x" /tmp/xev.out && ok=1
		kill $epid 2>/dev/null
		wait $epid 2>/dev/null
	fi
	kill $xpid 2>/dev/null
	wait $xpid 2>/dev/null
	[ $ok = 1 ] || cat /tmp/xev.out /tmp/xev-server.log
	[ $ok = 1 ]
}
t xev xev_keys
