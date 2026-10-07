# xev, installed as a package, on Xfbdev (the tinyx package), without a
# window manager: xev watches the root window's keyboard events (with no
# other window the keyboard's focus is the root), the host types "x"
# through the QEMU monitor and xev prints its KeyPress and KeyRelease.

xev_keys() {
	Xfbdev :0 -br > /tmp/xev-server.log 2>&1 &
	xpid=$!
	ok=0
	# xev gives up at once while the server does not answer yet.
	i=0
	while [ $i -lt 30 ]; do
		DISPLAY=:0 xev -root -event keyboard > /tmp/xev.out 2>&1 &
		epid=$!
		sleep 2
		kill -0 $epid 2>/dev/null && break
		i=$((i + 1))
	done
	if kill -0 $epid 2>/dev/null; then
		echo "HOST c-smokes sendkey x" >&3
		i=0
		while [ $i -lt 30 ] && ! grep -q KeyRelease /tmp/xev.out; do
			sleep 1
			i=$((i + 1))
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
