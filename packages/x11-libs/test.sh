# x11-libs, installed as a package: libX11 over libxcb against a stand-in
# server on /tmp/.X11-unix/X5 (x11_smoke.c), and libX11's error database.
t x11 sh -c '[ -f /lib/X11/XErrorDB ] && /bin/etc/x11_smoke'
