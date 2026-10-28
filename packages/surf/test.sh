# surf, installed as an app (with x11-xft, x11-fonts, dmenu and x11-apps),
# on Xfbdev: it prints its window's id (-w), opens the launcher's test page
# (src/test_www.rs; the network: `net`) and shows it, its title and its
# blue box (surf_check.c, an Xlib helper against mlibc as surf is); then,
# as its address bar (Ctrl-g) does, xprop sets the window's _SURF_GO and
# surf goes there (its _SURF_URI says so). surf keeps its cookies and
# cache under HOME.
SURF_URL=http://10.0.2.2:8766/hello.html

surf_run() {
	ok=0
	run-myos tinyx:Xfbdev :0 -br > /tmp/surf-server.log 2>&1 &
	xpid=$!
	mkdir -p /tmp/surf-home
	if run-myos surf:surf_check server; then
		HOME=/tmp/surf-home DISPLAY=:0 run-myos surf -w "$SURF_URL" > /tmp/surf.out 2> /tmp/surf.err &
		spid=$!
		i=0
		while [ $i -lt 120 ] && [ ! -s /tmp/surf.out ] && kill -0 $spid 2>/dev/null; do
			sleep 1
			i=$((i + 1))
		done
		xid=$(head -n 1 /tmp/surf.out)
		if [ -n "$xid" ] && run-myos surf:surf_check page "$xid" \
			&& DISPLAY=:0 run-myos x11-apps:xprop -id "$xid" -f _SURF_GO 8u -set _SURF_GO "$SURF_URL#go" \
			&& run-myos surf:surf_check uri "$xid" "$SURF_URL#go"; then
			ok=1
		fi
		kill $spid 2>/dev/null
		wait $spid 2>/dev/null
	fi
	kill $xpid 2>/dev/null
	wait $xpid 2>/dev/null
	# mlibc's lines for the sysdeps it lacks (issue #654) left out.
	[ $ok = 1 ] || { cat /tmp/surf.out; grep -v -e "missing sysdep" -e "Tag = " -e "fallback value" /tmp/surf.err \
		| tail -n 15; tail -n 5 /tmp/surf-server.log; }
	[ $ok = 1 ]
}
t -t net,netfs,x11,tinyx,x11-xft,x11-fonts,x11-apps,webkitgtk,surf,mlibc,heavy surf surf_run
