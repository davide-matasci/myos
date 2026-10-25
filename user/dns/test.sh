# The resolver over netd (QEMU's user network answers).
dns_lookup() {
	capture $OUT/dns.log dns github.com
	cat $OUT/dns.log
	contains "IP: " $OUT/dns.log && contains "[ OK ] dns" $OUT/dns.log
}
t -t netfs,virtio_net dns dns_lookup
