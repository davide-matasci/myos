# st, installed as an app, on Xfbdev (the tinyx package), without a
# window manager: st starts a shell on a pty; st_smoke gives st's window the
# focus, as a window manager would, and sees the shell's first line (lit
# pixels in the screen's top left, st_smoke.c); the host types a line
# through the QEMU monitor, st writes it to the pty and the shell saves it
# with st's TERM; st exits with it.

st_run() {
	run-myos tinyx:Xfbdev :0 -br > /tmp/st-server.log 2>&1 &
	xpid=$!
	ok=0
	rm -f /tmp/st-typed
	if run-myos st:st_smoke server; then
		DISPLAY=:0 run-myos st -e /bin/sh -c \
			'echo st-ready; read l; echo "$l $TERM" > /tmp/st-typed' > /tmp/st.log 2>&1 &
		spid=$!
		# The window first: until the server paints, the top left still
		# has the console's text.
		if run-myos st:st_smoke focus && run-myos st:st_smoke text; then
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
				run-myos st:st_smoke probe
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
