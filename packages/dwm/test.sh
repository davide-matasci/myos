# dwm, installed as a package, on Xfbdev (the tinyx package): it draws its
# bar (the selected tag's box in #005577 at the top left of the framebuffer),
# hides it on the Alt+B the host types through the QEMU monitor (the black
# root shows through, Xfbdev -br) and shows it again on a second Alt+B;
# dwm_smoke.c reads the pixel.

# On a failure: the /net conversations (a client that cannot reach the
# server's unix socket falls back to TCP), whether the server still answers
# a new client, and whether dwm was still running.
dwm_net_state() {
	for proto in unix tcp; do
		printf '/net/%s:' $proto
		for f in /net/$proto/[0-9]*/status; do
			[ -e "$f" ] && printf ' %s' "$(cat "$f")"
		done
		echo
	done
}

dwm_run() {
	Xfbdev :0 -br > /tmp/dwm-server.log 2>&1 &
	xpid=$!
	ok=0
	if /bin/etc/dwm_smoke server; then
		DISPLAY=:0 dwm > /tmp/dwm.log 2>&1 &
		dpid=$!
		if /bin/etc/dwm_smoke bar; then
			echo "HOST c-smokes sendkey alt-b" >&3
			if /bin/etc/dwm_smoke gone; then
				echo "HOST c-smokes sendkey alt-b" >&3
				/bin/etc/dwm_smoke bar && ok=1
			fi
		fi
		[ $ok = 1 ] || { dwm_net_state; /bin/etc/dwm_smoke probe; }
		kill $dpid 2>/dev/null
		wait $dpid 2>/dev/null
		# 143 (SIGTERM): dwm was still running.
		st=$?
		[ $ok = 1 ] || echo "dwm's exit status: $st"
	fi
	kill $xpid 2>/dev/null
	wait $xpid 2>/dev/null
	cat /tmp/dwm.log /tmp/dwm-server.log
	[ $ok = 1 ]
}
t dwm dwm_run
