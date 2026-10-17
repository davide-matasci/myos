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

# Joining the open network: open-system authentication and association
# with its access point, `status` naming it; a WPA2 network is refused (not
# supported yet), after the open one was left; joining again takes the next
# association id (the access point let the station go); `disconnect`
# leaves it.
wifi_connect() {
	/bin/custom/wifi daemon /dev/net1 &
	wifi_pid=$!
	wifi_rc=1
	for i in 1 2 3 4 5 6 7 8 9 10; do
		/bin/custom/wifi status > /dev/null 2>&1 && break
		sleep 1
	done
	want='associated 02:00:5e:00:01:01 1 myos-open aid 1'
	if [ "$(/bin/custom/wifi connect myos-open)" = "$want" ] &&
		[ "$(/bin/custom/wifi status)" = "$want" ] &&
		! /bin/custom/wifi connect myos-wpa2 > /tmp/wifi.err 2>&1 &&
		grep -q 'myos-wpa2 is wpa2-psk' /tmp/wifi.err &&
		[ "$(/bin/custom/wifi status)" = disconnected ] &&
		[ "$(/bin/custom/wifi connect myos-open)" = "${want%1}2" ] &&
		[ "$(/bin/custom/wifi disconnect)" = disconnected ] &&
		[ "$(/bin/custom/wifi status)" = disconnected ]; then
		wifi_rc=0
	else
		cat /tmp/wifi.err 2>/dev/null
		/bin/custom/wifi status
	fi
	kill $wifi_pid
	return $wifi_rc
}
t -t wifi wifi_connect wifi_connect

# The link's data path: associated to `myos-open`, the daemon holds a
# virtual NIC (/dev/tap) that netd takes, leasing 10.0.8.x from the access
# point's router (src/wifi_net.rs); a ping to the router crosses the link
# as 802.11 data frames, net0 keeps the default route, and the NIC goes
# with the link at `disconnect`.
wifi_data() {
	/bin/custom/wifi daemon /dev/net1 &
	wifi_pid=$!
	wifi_rc=1
	for i in 1 2 3 4 5 6 7 8 9 10; do
		/bin/custom/wifi status > /dev/null 2>&1 && break
		sleep 1
	done
	leased=0
	if /bin/custom/wifi connect myos-open > /tmp/wifi.err 2>&1; then
		for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15; do
			grep -q '^tap[0-3] 10\.0\.8\.[0-9]* 255\.255\.255\.0 ' /net/ifaddrs && leased=1 && break
			sleep 1
		done
	fi
	pinged=0
	if [ $leased = 1 ]; then
		for i in 1 2 3; do
			/bin/custom/ping 10.0.8.1 > /tmp/wifi.ping 2>&1 && pinged=1 && break
		done
	fi
	if [ $pinged = 1 ] && ! grep -q 'ip=10\.0\.8\.' /net/ndb &&
		[ "$(/bin/custom/wifi disconnect)" = disconnected ]; then
		for i in 1 2 3 4 5; do
			grep -q '^tap' /net/ifaddrs || { wifi_rc=0; break; }
			sleep 1
		done
	fi
	if [ $wifi_rc != 0 ]; then
		echo "leased $leased pinged $pinged"
		cat /tmp/wifi.err /tmp/wifi.ping /net/ifaddrs /net/ndb 2>/dev/null
		for c in /dev/tap/*/ctl; do echo "$c:"; cat "$c"; done 2>/dev/null
		ps 2>/dev/null | grep -e netd -e wifi
		/bin/custom/wifi status
	fi
	kill $wifi_pid
	return $wifi_rc
}
t -t wifi,tap wifi_data wifi_data
