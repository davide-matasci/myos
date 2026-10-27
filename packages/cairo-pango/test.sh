# cairo-pango, installed as an app with tinyx, x11-xft (fonts.conf) and
# x11-fonts: fribidi and harfbuzz, pango's text through cairo into a PNG
# that gdk-pixbuf loads back (and a JPEG through memory), and cairo's Xlib
# surface on Xfbdev (smoke.c, against mlibc).
cairo_pango_run() {
	run-myos tinyx:Xfbdev :0 -br > /tmp/cairo-pango-server.log 2>&1 &
	xpid=$!
	run-myos cairo-pango,x11-xft,x11-fonts:cairo-pango-smoke > /tmp/cairo-pango.out 2>&1
	rc=$?
	kill $xpid 2>/dev/null
	wait $xpid 2>/dev/null
	[ $rc = 0 ] || cat /tmp/cairo-pango.out /tmp/cairo-pango-server.log
	return $rc
}
t -t x11,tinyx,x11-xft,x11-fonts,cairo-pango,mlibc cairo_pango cairo_pango_run
