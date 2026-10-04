# dmenu, installed as a package, on Xfbdev (the tinyx package), without a
# window manager; the host types through the QEMU monitor (dmenu grabs the
# keyboard).
#   dmenu: its bar shows the selected item (#005577 along the screen's top,
#     dmenu_smoke.c); "b" and Return select "beta", which it prints.
#   dmenu_run: dwm's Alt+P. dmenu_path lists the programs of PATH (stest,
#     the list cached under XDG_CACHE_HOME), "zz" and Return pick a #!
#     script there and the shell dmenu_run pipes into runs it.

dmenu_server() {
	Xfbdev :0 -br > /tmp/dmenu-server.log 2>&1 &
	xpid=$!
	/bin/etc/dmenu_smoke server
}

dmenu_server_stop() {
	kill $xpid 2>/dev/null
	wait $xpid 2>/dev/null
}

dmenu_pick() {
	ok=0
	if dmenu_server; then
		printf 'alpha\nbeta\ngamma\n' | DISPLAY=:0 dmenu > /tmp/dmenu.out 2> /tmp/dmenu.log &
		dpid=$!
		if /bin/etc/dmenu_smoke bar; then
			echo "HOST c-smokes sendkey b ret" >&3
			wait $dpid
			st=$?
			out=$(cat /tmp/dmenu.out)
			echo "dmenu exited with $st and printed: $out"
			[ $st = 0 ] && [ "$out" = beta ] && ok=1
		fi
		[ $ok = 1 ] || { kill $dpid 2>/dev/null; wait $dpid 2>/dev/null; }
	fi
	dmenu_server_stop
	cat /tmp/dmenu.log /tmp/dmenu-server.log
	[ $ok = 1 ]
}
t dmenu dmenu_pick

dmenu_run_pick() {
	ok=0
	mkdir -p /tmp/dmenu-bin /tmp/dmenu-cache
	printf '#!/bin/sh\necho ran > /tmp/dmenu-ran\n' > /tmp/dmenu-bin/zzdmenu
	chmod +x /tmp/dmenu-bin/zzdmenu
	rm -f /tmp/dmenu-ran /tmp/dmenu-cache/dmenu_run
	listed=$(PATH=/tmp/dmenu-bin:$PATH XDG_CACHE_HOME=/tmp/dmenu-cache dmenu_path | grep -x -e zzdmenu -e dmenu -e sh)
	echo "dmenu_path lists" $listed
	if [ "$(echo "$listed" | wc -l)" -eq 3 ] && dmenu_server; then
		PATH=/tmp/dmenu-bin:$PATH XDG_CACHE_HOME=/tmp/dmenu-cache DISPLAY=:0 dmenu_run 2> /tmp/dmenu.log
		if /bin/etc/dmenu_smoke bar; then
			echo "HOST c-smokes sendkey z z ret" >&3
			for i in 1 2 3 4 5 6 7 8 9 10; do
				[ -f /tmp/dmenu-ran ] && break
				sleep 1
			done
			[ "$(cat /tmp/dmenu-ran 2>/dev/null)" = ran ] && ok=1
		fi
		[ $ok = 1 ] || echo "the script did not run"
		dmenu_server_stop
	fi
	cat /tmp/dmenu.log /tmp/dmenu-server.log
	[ $ok = 1 ]
}
t dmenu_run dmenu_run_pick
