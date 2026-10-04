# st, installed as a package, on Xfbdev (the tinyx package), without a
# window manager: st, sized to cover the screen's centre, starts a shell on
# a pty that prints a line (lit pixels in the screen's top left,
# st_smoke.c), reads what the host types through the QEMU monitor (the keys
# reach st, the window under the pointer, and st writes them to the pty)
# and saves it with st's TERM; st exits with it.

st_run() {
	Xfbdev :0 -br > /tmp/st-server.log 2>&1 &
	xpid=$!
	ok=0
	rm -f /tmp/st-typed
	if /bin/etc/st_smoke server; then
		# Big enough to cover the pointer, at the screen's centre (st's
		# cells are about 7x15 pixels), not much more: st draws through a
		# pixmap of its size.
		read -r w h rest < /dev/fb/ctl
		DISPLAY=:0 st -g $((w / 12 + 8))x$((h / 24 + 6)) -e /bin/sh -c \
			'echo st-ready; read l; echo "$l $TERM" > /tmp/st-typed' > /tmp/st.log 2>&1 &
		spid=$!
		if /bin/etc/st_smoke text; then
			echo "HOST c-smokes sendkey o k ret" >&3
			i=0
			while [ $i -lt 60 ] && [ ! -s /tmp/st-typed ]; do
				sleep 1
				i=$((i + 1))
			done
			typed=$(cat /tmp/st-typed 2>/dev/null)
			echo "the shell got: $typed"
			if [ "$typed" = "ok st-256color" ]; then
				# st exits with its shell.
				wait $spid
				st=$?
				echo "st exited with $st"
				[ $st = 0 ] && ok=1
			else
				/bin/etc/st_smoke probe
			fi
		fi
		[ $ok = 1 ] || { kill $spid 2>/dev/null; wait $spid 2>/dev/null; }
	fi
	kill $xpid 2>/dev/null
	wait $xpid 2>/dev/null
	cat /tmp/st.log /tmp/st-server.log
	[ $ok = 1 ]
}
t st st_run
