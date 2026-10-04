# x11-xft, installed as a package with x11-fonts and tinyx: fontconfig finds
# the fonts in /lib/fonts and resolves "monospace" to DejaVu Sans Mono; on
# Xfbdev, xft_smoke draws antialiased text with Xft and reads it back from
# the framebuffer (xft_smoke.c).
xft_run() {
	fc-match monospace > /tmp/xft.out 2>&1
	if ! contains "DejaVu Sans Mono" /tmp/xft.out; then
		cat /tmp/xft.out
		return 1
	fi
	Xfbdev :0 -br > /tmp/xft-server.log 2>&1 &
	xpid=$!
	/bin/etc/xft_smoke >> /tmp/xft.out 2>&1
	rc=$?
	kill $xpid 2>/dev/null
	wait $xpid 2>/dev/null
	[ $rc = 0 ] || cat /tmp/xft.out /tmp/xft-server.log
	return $rc
}
t xft xft_run
