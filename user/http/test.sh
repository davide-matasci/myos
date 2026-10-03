# HTTPS GET with the kernel's own client (TLS, the wall clock). Full mode
# only: it needs the real network.
[ "$MODE" = full ] || return 0

https_get() {
	capture $OUT/http.log http https://example.com/
	tail -n 5 $OUT/http.log
	contains "Example Domain" $OUT/http.log && contains "[ OK ] https" $OUT/http.log
}
t https https_get
