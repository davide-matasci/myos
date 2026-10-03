# The network (netd, the resolver, TLS) and listen/accept.

net_dns() {
	capture $OUT/dns.log dns www.google.com
	cat $OUT/dns.log
	contains "IP: " $OUT/dns.log && contains "[ OK ] dns" $OUT/dns.log
}
# HTTPS GET with the kernel's own client (user/http: TLS, the wall clock).
net_https() {
	capture $OUT/http.log http https://example.com/
	tail -n 5 $OUT/http.log
	contains "Example Domain" $OUT/http.log && contains "[ OK ] https" $OUT/http.log
}
# netd listen/accept: the smoke announces TCP 2323; the host connects back
# through QEMU's port forward (the HOST request), sends "ping" and expects
# "pong"; the smoke then reports.
net_listen() {
	: > /tmp/listen.out
	/bin/etc/tcp_listen_smoke >> /tmp/listen.out 2>&1 &
	pid=$!
	echo "HOST tcp-ping 2323" >&3
	i=0
	while [ $i -lt 180 ] && ! grep -q -e "OK ] listen" -e "FAIL ]" /tmp/listen.out 2>/dev/null; do
		sleep 1
		i=$((i + 1))
	done
	kill $pid 2>/dev/null
	wait $pid 2>/dev/null
	cat /tmp/listen.out
	contains "[ OK ] listen" /tmp/listen.out
}

t dns net_dns
if [ "$MODE" = full ]; then
	t https net_https
fi
t listen net_listen
