# /ping over netd's /net/icmp: an ICMPv6 echo to ::1, which the loopback
# interface answers (ipv6_smoke pings the router too).
ping_ipv6() {
	capture $OUT/ping6.log ping ::1 || { cat $OUT/ping6.log; return 1; }
	contains "[ OK ] ping" $OUT/ping6.log
}
t -t netfs,ipv6 ping_ipv6 ping_ipv6
