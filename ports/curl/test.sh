# curl over the userspace sockets and LibreSSL: an HTTPS GET from the
# launcher's test server (src/test_www.rs; the network: `net`), its
# certificate checked against the test CA alone.

curl_https() {
	curl -fsS --connect-timeout 30 --max-time 90 --cacert /lib/myos-tests/test-ca.pem \
		-o /tmp/curl-hello.txt https://10.0.2.2:8767/hello.txt || return 1
	contains "hello from the myos test server" /tmp/curl-hello.txt
}
t -t net,netfs curl_https curl_https
