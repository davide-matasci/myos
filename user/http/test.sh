# HTTPS GET with the kernel's own client (TLS, the wall clock): GitHub's
# robots.txt, a small page, its certificate checked against the image's
# bundle. It needs the real network (`net`); GitHub because CI cannot run
# without it anyway.

https_get() {
	capture $OUT/http.log http https://github.com/robots.txt
	tail -n 5 $OUT/http.log
	contains "User-agent" $OUT/http.log && contains "[ OK ] https" $OUT/http.log
}
t -t net,netfs https https_get
