# image-libs, installed as an app with tinyx: pixman draws, libpng,
# libwebp and libjpeg-turbo round-trip the image through memory, and on
# Xfbdev the image goes up over MIT-SHM, RENDER fills beside it, Xcursor
# makes a cursor, and the pixels come back through XGetImage (smoke.c,
# against mlibc and x11-libs' mlibc build).
image_run() {
	run-myos tinyx:Xfbdev :0 -br > /tmp/image-server.log 2>&1 &
	xpid=$!
	run-myos image-libs:image-smoke > /tmp/image.out 2>&1
	rc=$?
	kill $xpid 2>/dev/null
	wait $xpid 2>/dev/null
	[ $rc = 0 ] || cat /tmp/image.out /tmp/image-server.log
	return $rc
}
t -t x11,tinyx,image-libs,mlibc image image_run
