# Building an X client on myos, as a suckless program's Makefile does:
# make's default $(CC), cc (tcc), compiles and links it with `-lX11` alone
# against the x11-dev package (tcc searches /lib/x11). The client names
# the root window, as dwmblocks sets dwm's status, on Xfbdev (the tinyx
# package), and xprop (x11-apps) reads the name back. The server answers
# once it holds the screen: the client is retried.
x11_dev_make() {
	mkdir -p /tmp/xname
	cat > /tmp/xname/xname.c <<'C'
#include <X11/Xlib.h>
int main(void) {
	Display *d = XOpenDisplay(NULL);
	if (!d)
		return 1;
	XStoreName(d, DefaultRootWindow(d), "built on myos");
	XCloseDisplay(d);
	return 0;
}
C
	printf 'xname: xname.c\n\t$(CC) -o $@ xname.c -lX11\n' > /tmp/xname/Makefile
	run-myos tcc,x11-dev,make:make -C /tmp/xname > /tmp/xname.out 2>&1 \
		|| { cat /tmp/xname.out; return 1; }
	run-myos tinyx:Xfbdev :0 -br > /tmp/xname-server.log 2>&1 &
	xpid=$!
	i=0
	while [ $i -lt 60 ] && ! DISPLAY=:0 /tmp/xname/xname >> /tmp/xname.out 2>&1; do
		sleep 1
		i=$((i + 1))
	done
	DISPLAY=:0 run-myos x11-apps:xprop -root WM_NAME >> /tmp/xname.out 2>&1
	grep -qx 'WM_NAME(STRING) = "built on myos"' /tmp/xname.out
	ok=$?
	kill $xpid 2>/dev/null
	wait $xpid 2>/dev/null
	[ $ok = 0 ] || cat /tmp/xname.out /tmp/xname-server.log
	rm -r /tmp/xname
	return $ok
}
t -t x11,tinyx,tcc,make,x11-apps x11_dev_make x11_dev_make
