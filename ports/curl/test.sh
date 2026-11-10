# curl over the userspace sockets and LibreSSL: an HTTPS GET from the
# launcher's test server (src/test_www.rs; the network: `net`), its
# certificate checked against the test CA alone.

curl_https() {
	curl -fsS --connect-timeout 30 --max-time 90 --cacert /lib/myos-tests/test-ca.pem \
		-o /tmp/curl-hello.txt https://10.0.2.2:8767/hello.txt || return 1
	contains "hello from the myos test server" /tmp/curl-hello.txt
}
t -t net,netfs curl_https curl_https

# A connect QEMU's user network cannot make, to its DNS servers on a port
# other than 53, comes back as an ICMP or ICMPv6 "destination unreachable"
# on every host: it fails at once (7, could not connect), as on Linux,
# instead of waiting out curl's timeout (28). A host without IPv6 answers
# every IPv6 connect so, and curl tried those first.
curl_unreachable() {
	for a in 10.0.2.4 '[fec0::3]'; do
		curl -sS --connect-timeout 30 -o /dev/null http://$a:80/ 2>/dev/null
		r=$?
		[ $r -eq 7 ] || { echo "curl http://$a:80/: exit $r, not 7"; return 1; }
	done
}
t -t netfs curl_unreachable curl_unreachable
