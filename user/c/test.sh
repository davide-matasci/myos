# The C smokes: ptys (openpty round trip through the line discipline, EIO
# once the slave closes) and /dev/urandom (non-zero, distinct, changing).
t pty /bin/etc/pty_smoke 2
t urandom /bin/etc/urandom_smoke
# The tty: the line editor's keys and ^C, driven through a pty (tty_smoke.c).
t tty /bin/etc/tty_smoke
# AF_UNIX over /net/unix: socketpair, listen/accept/connect with a forked
# client, a transfer larger than the buffers, EOF, names (unix_smoke.c).
t unix /bin/etc/unix_smoke
# /dev/fb: the geometry in ctl, a MAP_SHARED mapping of data that is the
# framebuffer itself (shared with read/write and a forked child), a
# MAP_PRIVATE one that is a copy, and the screen handed to a program and back
# by `text`, the last close of ctl and its holder's exit (fb_smoke.c).
t fb /bin/etc/fb_smoke
# poll: timeouts, waking on pipe and unix socket events (not before),
# POLLHUP/POLLERR/POLLNVAL, EAGAIN, EINTR (poll_smoke.c).
t poll /bin/etc/poll_smoke
# netd listen/accept: the smoke announces TCP 2323; the host connects back
# through QEMU's port forward (the HOST request runs host.sh tcp-ping),
# sends "ping" and expects "pong"; the smoke then reports.
net_listen() {
	: > /tmp/listen.out
	/bin/etc/tcp_listen_smoke >> /tmp/listen.out 2>&1 &
	pid=$!
	echo "HOST c-smokes tcp-ping 2323" >&3
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
t listen net_listen
