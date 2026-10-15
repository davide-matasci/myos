# HTTPS GET with the kernel's own client (TLS, the wall clock). It needs
# the real network (`net`).

https_get() {
	capture $OUT/http.log http https://example.com/
	tail -n 5 $OUT/http.log
	contains "Example Domain" $OUT/http.log && contains "[ OK ] https" $OUT/http.log
}
t -t net,netfs https https_get
