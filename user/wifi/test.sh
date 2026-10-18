# Wi-Fi over the test boots' simulated radio (src/wifi_sim.rs): a scan
# of the 2.4 GHz channels finds the launcher's three access points, each
# on its channel with its signal and security, the strongest first, and
# `wifi list` gives the same afterwards (docs/wifi.md). The daemon is
# started with no NIC: with no AX201 in /dev/pci it takes /dev/net1's.
wifi_scan() {
	/bin/custom/wifi daemon &
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
# with its access point, `status` naming it; the WPA2 network refused
# without a passphrase (after the open one was left) and with a wrong one
# (message 2's MIC: the access point lets the station go); joining again
# takes the next association id (the access point let the station go);
# the WPA2 network joined with its passphrase (src/wifi_sim.rs), then
# `disconnect` leaves it.
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
		grep -q 'myos-wpa2 is wpa2-psk: connect SSID PASSPHRASE' /tmp/wifi.err &&
		[ "$(/bin/custom/wifi status)" = disconnected ] &&
		! /bin/custom/wifi connect myos-wpa2 not-the-passphrase > /tmp/wifi.err 2>&1 &&
		grep -q 'wrong passphrase' /tmp/wifi.err &&
		[ "$(/bin/custom/wifi status)" = disconnected ] &&
		[ "$(/bin/custom/wifi connect myos-open)" = "${want%1}3" ] &&
		[ "$(/bin/custom/wifi connect myos-wpa2 myos-passphrase)" = 'associated 02:00:5e:00:01:06 6 myos-wpa2 aid 4' ] &&
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

# The link's data path: associated to SSID (with PASSPHRASE on a WPA2
# network), the daemon holds a virtual NIC (/dev/tap) that netd takes,
# leasing 10.0.8.x from the access point's router (src/wifi_net.rs); a
# ping to the router crosses the link as 802.11 data frames, net0 keeps
# the default route, and the NIC goes with the link at `disconnect`. On
# the WPA2 network every data frame is CCMP's, the router's broadcast
# DHCP answers under the group key, and the access point rekeys the group
# at the station's first frame (a group key handshake).
wifi_data() {
	/bin/custom/wifi daemon /dev/net1 &
	wifi_pid=$!
	wifi_rc=1
	for i in 1 2 3 4 5 6 7 8 9 10; do
		/bin/custom/wifi status > /dev/null 2>&1 && break
		sleep 1
	done
	leased=0
	if /bin/custom/wifi connect "$@" > /tmp/wifi.err 2>&1; then
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
t -t wifi,tap wifi_data wifi_data myos-open
t -t wifi,tap wifi_wpa2_data wifi_data myos-wpa2 myos-passphrase

# wifi-firmware: a file whose SHA-256 is not the pinned one is refused and
# not kept (a mirror directory with the wrong bytes); `list` says it is
# missing.
wifi_firmware_check() {
	fw=/tmp/fw-check
	rm -rf $fw $fw.mirror
	mkdir -p $fw.mirror/intel/iwlwifi
	echo "not firmware" > $fw.mirror/intel/iwlwifi/iwlwifi-QuZ-a0-hr-b0-77.ucode
	if MYOS_FIRMWARE=$fw WIFI_FIRMWARE_MIRROR=$fw.mirror /bin/custom/wifi-firmware fetch > $fw.out 2>&1; then
		cat $fw.out
		return 1
	fi
	grep -q 'wrong SHA-256' $fw.out && [ -z "$(ls -A $fw)" ] &&
		[ "$(MYOS_FIRMWARE=$fw /bin/custom/wifi-firmware list)" = "iwlwifi-QuZ-a0-hr-b0-77.ucode missing" ] ||
		{ cat $fw.out; ls -la $fw; return 1; }
}
t -t wifi wifi_firmware_check wifi_firmware_check

# wifi-firmware fetch: the pinned AX201 firmware from linux-firmware, its
# SHA-256 checked; a second fetch keeps it.
wifi_firmware_fetch() {
	fw=/tmp/fw-fetch
	rm -rf $fw
	MYOS_FIRMWARE=$fw /bin/custom/wifi-firmware fetch > $fw.out 2>&1 &&
		grep -q '^fetched iwlwifi-QuZ-a0-hr-b0-77.ucode$' $fw.out &&
		[ "$(MYOS_FIRMWARE=$fw /bin/custom/wifi-firmware fetch)" = "have iwlwifi-QuZ-a0-hr-b0-77.ucode" ] &&
		[ "$(MYOS_FIRMWARE=$fw /bin/custom/wifi-firmware list)" = "iwlwifi-QuZ-a0-hr-b0-77.ucode ok" ] ||
		{ cat $fw.out; return 1; }
	rm -rf $fw
}
t -t wifi,net wifi_firmware_fetch wifi_firmware_fetch
