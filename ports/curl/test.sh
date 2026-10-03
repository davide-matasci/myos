# curl over the userspace sockets and mbedtls: an HTTPS GET of example.com
# (the network: full mode only).
[ "$MODE" = full ] || return 0

curl_https() {
	curl -fsS --connect-timeout 30 --max-time 90 -o /tmp/curl-ex.html https://example.com/ || return 1
	contains "Example Domain" /tmp/curl-ex.html
}
t curl_https curl_https
