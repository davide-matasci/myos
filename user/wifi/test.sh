# Wi-Fi over the test boots' simulated radio (src/wifi_sim.rs): a scan
# of the 2.4 GHz channels finds the launcher's three access points, each
# on its channel with its signal and security, the strongest first, and
# `wifi list` gives the same afterwards (docs/wifi.md).
wifi_scan() {
	/bin/custom/wifi daemon /dev/net1 &
	wifi_pid=$!
	wifi_rc=1
	# The daemon's socket is there once it announced it.
	for i in 1 2 3 4 5 6 7 8 9 10; do
		/bin/custom/wifi list > /tmp/wifi.list 2>&1 && break
		sleep 1
	done
	if /bin/custom/wifi scan > /tmp/wifi.scan 2>&1; then
		cat /tmp/wifi.scan
		printf '%s\n' \
			'02:00:5e:00:01:01 1 -40 open myos-open' \
			'02:00:5e:00:01:06 6 -55 wpa2-psk myos-wpa2' \
			'02:00:5e:00:01:0b 11 -82 wpa2-psk myos-far' > /tmp/wifi.want
		/bin/custom/wifi list > /tmp/wifi.list 2>&1
		cmp /tmp/wifi.want /tmp/wifi.scan && cmp /tmp/wifi.want /tmp/wifi.list && wifi_rc=0
	fi
	kill $wifi_pid
	return $wifi_rc
}
t -t wifi wifi_scan wifi_scan
