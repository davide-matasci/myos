# webkitgtk, installed as an app with tinyx, x11-xft (fonts.conf)
# and x11-fonts: a web view on Xfbdev loads the launcher's test page
# (src/test_www.rs; the network: `net`), its title, its script's work
# and its drawing checked (smoke.c, against mlibc: also WebKit's web and
# network processes).
webkitgtk_run() {
	run-myos tinyx:Xfbdev :0 -br > /tmp/webkitgtk-server.log 2>&1 &
	xpid=$!
	run-myos webkitgtk,x11-xft,x11-fonts:webkitgtk-smoke > /tmp/webkitgtk.out 2>&1
	rc=$?
	kill $xpid 2>/dev/null
	wait $xpid 2>/dev/null
	[ $rc = 0 ] || cat /tmp/webkitgtk.out /tmp/webkitgtk-server.log
	return $rc
}
t -t net,netfs,x11,tinyx,x11-xft,x11-fonts,webkitgtk,mlibc,heavy webkitgtk webkitgtk_run
