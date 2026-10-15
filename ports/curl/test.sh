# curl over the userspace sockets and mbedtls: an HTTPS GET of example.com
# (the network: `net`).

curl_https() {
	curl -fsS --connect-timeout 30 --max-time 90 -o /tmp/curl-ex.html https://example.com/ || return 1
	contains "Example Domain" /tmp/curl-ex.html
}
t -t net,netfs curl_https curl_https
