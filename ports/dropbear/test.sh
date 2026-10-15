# dropbear: the host opens two SSH sessions at once through QEMU's port
# forward (the HOST request runs host.sh) to the dropbear the boot script
# started (etc/rc.local: `dropbear -R -D /etc/dropbear -p 2222`, as
# docs/ssh.md says), pubkey auth with the test key, put in its
# authorized_keys for the test only; the host key it makes at the first
# connection is kept in /etc/dropbear (/data/dropbear, bound over it).
# Each session writes its PATH to a file in /tmp when it gets in: myos's
# directories (DEFAULT_ROOT_PATH in localoptions.h; the shell keeps what it
# inherits). Then a login on a pty: the pair is its session's controlling
# terminal, /proc/self/tty names it and /dev/tty opens. The host needs an
# ssh client (the CI container has one).

ssh_two_clients() {
	rm -f /tmp/ssh-ok-a /tmp/ssh-ok-b /tmp/ssh-tty
	# The image has no key: the test's own, for this test only.
	cp /lib/myos-tests/dropbear-testkey.pub /etc/dropbear/authorized_keys || return 1
	echo "HOST dropbear 2222" >&3
	i=0
	while [ $i -lt 300 ] && ! { [ -f /tmp/ssh-ok-a ] && [ -f /tmp/ssh-ok-b ] && [ -f /tmp/ssh-tty ]; }; do
		sleep 1
		i=$((i + 1))
	done
	rm /etc/dropbear/authorized_keys
	ls /etc/dropbear
	[ -f /tmp/ssh-ok-a ] && [ -f /tmp/ssh-ok-b ] && [ -f /tmp/ssh-tty ] || { ps; return 1; }
	[ -s /etc/dropbear/dropbear_ed25519_host_key ] || return 1
	cat /tmp/ssh-ok-a /tmp/ssh-tty
	[ "$(cat /tmp/ssh-ok-a)" = /bin/sbase:/bin/coreutils:/bin/ubase:/bin/custom:/bin/tcc:/bin/std:/bin/etc ] || return 1
	case "$(cat /tmp/ssh-tty)" in
	/dev/pts/[0-9]*tty-ok) ;;
	*) return 1 ;;
	esac
}
t -t netfs,tty ssh ssh_two_clients
