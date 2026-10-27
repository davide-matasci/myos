# gtk3, installed as an app with tinyx, x11-xft (fonts.conf) and x11-fonts:
# a GTK window with a label, a button and an entry on Xfbdev, its signals
# and its drawn text read back (smoke.c, against mlibc).
gtk3_run() {
	run-myos tinyx:Xfbdev :0 -br > /tmp/gtk3-server.log 2>&1 &
	xpid=$!
	run-myos gtk3,x11-xft,x11-fonts:gtk3-smoke > /tmp/gtk3.out 2>&1
	rc=$?
	kill $xpid 2>/dev/null
	wait $xpid 2>/dev/null
	[ $rc = 0 ] || cat /tmp/gtk3.out /tmp/gtk3-server.log
	return $rc
}
t -t x11,tinyx,x11-xft,x11-fonts,gtk3,mlibc gtk3 gtk3_run
