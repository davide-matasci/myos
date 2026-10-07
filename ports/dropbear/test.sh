# dropbear: the host opens two SSH sessions at once through QEMU's port
# forward (the HOST request runs host.sh; pubkey auth with the test key);
# each session writes its PATH to a file in /tmp when it gets in: myos's
# directories (DEFAULT_ROOT_PATH in localoptions.h; the shell keeps what it
# inherits). Full mode only (the host needs an ssh client and the time).
[ "$MODE" = full ] || return 0

ssh_two_clients() {
	rm -f /tmp/ssh-ok-a /tmp/ssh-ok-b
	: > /tmp/dropbear.out
	/bin/custom/dropbear -F -E -p 22 -r /etc/dropbear/ed25519_hostkey >> /tmp/dropbear.out 2>&1 &
	pid=$!
	echo "HOST dropbear 2222" >&3
	i=0
	while [ $i -lt 300 ] && ! { [ -f /tmp/ssh-ok-a ] && [ -f /tmp/ssh-ok-b ]; }; do
		sleep 1
		i=$((i + 1))
	done
	kill $pid 2>/dev/null
	wait $pid 2>/dev/null
	cat /tmp/dropbear.out
	[ -f /tmp/ssh-ok-a ] && [ -f /tmp/ssh-ok-b ] || return 1
	cat /tmp/ssh-ok-a
	[ "$(cat /tmp/ssh-ok-a)" = /bin/sbase:/bin/coreutils:/bin/ubase:/bin/custom:/bin/tcc:/bin/std:/bin/etc ]
}
t ssh ssh_two_clients
