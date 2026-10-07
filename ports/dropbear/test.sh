# dropbear: the host opens two SSH sessions at once through QEMU's port
# forward (the HOST request runs host.sh; pubkey auth with the test key,
# the image's root being read-only: an authorized_keys and a host key in
# /tmp, dropbear -D and -r, as docs/ssh.md says to start it);
# each session writes its PATH to a file in /tmp when it gets in: myos's
# directories (DEFAULT_ROOT_PATH in localoptions.h; the shell keeps what it
# inherits). Then a login on a pty: the pair is its session's controlling
# terminal, /proc/self/tty names it and /dev/tty opens. Full mode only (the
# host needs an ssh client and the time).
[ "$MODE" = full ] || return 0

ssh_two_clients() {
	rm -rf /tmp/ssh-ok-a /tmp/ssh-ok-b /tmp/ssh-tty /tmp/ssh-test
	: > /tmp/dropbear.out
	# The image has no key: the test's own, for this test only.
	mkdir -p /tmp/ssh-test
	cp /lib/myos-tests/dropbear-testkey.pub /tmp/ssh-test/authorized_keys || return 1
	/bin/custom/dropbearkey -t ed25519 -f /tmp/ssh-test/hostkey >> /tmp/dropbear.out 2>&1 || return 1
	/bin/custom/dropbear -F -E -p 22 -r /tmp/ssh-test/hostkey -D /tmp/ssh-test >> /tmp/dropbear.out 2>&1 &
	pid=$!
	echo "HOST dropbear 2222" >&3
	i=0
	while [ $i -lt 300 ] && ! { [ -f /tmp/ssh-ok-a ] && [ -f /tmp/ssh-ok-b ] && [ -f /tmp/ssh-tty ]; }; do
		sleep 1
		i=$((i + 1))
	done
	kill $pid 2>/dev/null
	wait $pid 2>/dev/null
	rm -rf /tmp/ssh-test
	cat /tmp/dropbear.out
	[ -f /tmp/ssh-ok-a ] && [ -f /tmp/ssh-ok-b ] && [ -f /tmp/ssh-tty ] || return 1
	cat /tmp/ssh-ok-a /tmp/ssh-tty
	[ "$(cat /tmp/ssh-ok-a)" = /bin/sbase:/bin/coreutils:/bin/ubase:/bin/custom:/bin/tcc:/bin/std:/bin/etc ] || return 1
	case "$(cat /tmp/ssh-tty)" in
	/dev/pts/[0-9]*tty-ok) ;;
	*) return 1 ;;
	esac
}
t ssh ssh_two_clients
