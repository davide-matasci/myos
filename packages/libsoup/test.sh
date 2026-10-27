# libsoup, installed as an app: GETs from the launcher's test server
# (src/test_www.rs; the network: `net`) over HTTP and over HTTPS through
# glib-networking and LibreSSL, the certificate checked against the test
# CA, and refused without it; libxml2 and libpsl (smoke.c, against mlibc).
libsoup_run() {
	run-myos libsoup:libsoup-smoke > /tmp/libsoup.out 2>&1 || {
		cat /tmp/libsoup.out
		return 1
	}
}
t -t net,netfs,pkg,libsoup,mlibc libsoup libsoup_run
