# x11-libs, installed as an app: libX11 over libxcb against a stand-in
# server on /tmp/.X11-unix/X5 (x11_smoke.c), libX11's error database and
# the C locale's Compose file (dmenu's XOpenIM opens its input method); the
# client's threads share its display (Xlib's constructor called
# XInitThreads).
t x11 run-myos x11-libs:/bin/sh -c '[ -f /lib/X11/XErrorDB ] && [ -f /lib/X11/locale/iso8859-1/Compose ] && /bin/etc/x11_smoke'
