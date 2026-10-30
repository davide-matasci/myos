# x11-xft, installed as an app with x11-fonts and tinyx: fontconfig finds
# the fonts in /lib/fonts and resolves "monospace" to DejaVu Sans Mono, so
# does st's "Liberation Mono" (not the closest name, DejaVu Sans), and a
# braille pattern, which Mono lacks, to DejaVu Sans (btm's graphs); on
# Xfbdev, xft_smoke draws antialiased text with Xft and reads it back from
# the framebuffer (xft_smoke.c).
xft_run() {
	run-myos x11-xft,x11-fonts:fc-match monospace > /tmp/xft.out 2>&1
	run-myos x11-xft,x11-fonts:fc-match 'monospace:charset=2847' > /tmp/xft-braille.out 2>&1
	run-myos x11-xft,x11-fonts:fc-match 'Liberation Mono:pixelsize=12' > /tmp/xft-st.out 2>&1
	if ! contains "DejaVu Sans Mono" /tmp/xft.out \
		|| ! contains '"DejaVu Sans"' /tmp/xft-braille.out \
		|| ! contains "DejaVu Sans Mono" /tmp/xft-st.out; then
		cat /tmp/xft.out /tmp/xft-braille.out /tmp/xft-st.out
		return 1
	fi
	run-myos tinyx:Xfbdev :0 -br > /tmp/xft-server.log 2>&1 &
	xpid=$!
	run-myos x11-xft,x11-fonts:xft_smoke >> /tmp/xft.out 2>&1
	rc=$?
	kill $xpid 2>/dev/null
	wait $xpid 2>/dev/null
	[ $rc = 0 ] || cat /tmp/xft.out /tmp/xft-server.log
	return $rc
}
t -t x11,tinyx,x11-fonts xft xft_run
