# The C smokes: ptys (openpty round trip through the line discipline, EIO
# once the slave closes) and /dev/urandom (non-zero, distinct, changing).
t pty /bin/etc/pty_smoke 2
# More pty pairs at once than the kernel once had (four), their names and
# inodes distinct, a closed pair's index used again (pty_smoke.c).
t ptys /bin/etc/pty_smoke 4
# Writers faster than their reader: a write into a full pty waits for room,
# also on the reader's CPU (pty_smoke.c).
t pty_flood /bin/etc/pty_smoke 5
t urandom /bin/etc/urandom_smoke
# The tty: the line editor's keys and ^C, driven through a pty (tty_smoke.c).
t tty /bin/etc/tty_smoke
# AF_UNIX over /net/unix: socketpair, listen/accept/connect with a forked
# client, a transfer larger than the buffers, 60000 bytes ahead of the reader,
# 48 conversations at once, EOF, names (unix_smoke.c).
t unix /bin/etc/unix_smoke
# /dev/fb: the geometry in ctl, a MAP_SHARED mapping of data that is the
# framebuffer itself (shared with read/write and a forked child), a
# MAP_PRIVATE one that is a copy, and the screen handed to a program and back
# by `text`, the last close of ctl and its holder's exit (fb_smoke.c).
t fb /bin/etc/fb_smoke
# The text console's UTF-8: box-drawing, block and braille characters drawn
# in one cell each, one without a glyph a single '?', a wide one two cells,
# a combining mark none; 256 colors and RGB (fb_smoke.c).
t fb_text /bin/etc/fb_smoke text
# poll: timeouts, waking on pipe and unix socket events (not before),
# POLLHUP/POLLERR/POLLNVAL, EAGAIN, EINTR (poll_smoke.c).
t poll /bin/etc/poll_smoke
# readv/writev over a pipe, a socketpair and a nonblocking socket that
# fills mid-writev (uio_smoke.c).
t uio /bin/etc/uio_smoke
# POSIX threads: the API on one thread, then threads contending for a
# mutex and a condition variable, errno, keys, stdio and malloc per thread,
# flockfile, readdir_r, 150 threads started and joined, read-write locks,
# barriers, spin locks, a CLOCK_MONOTONIC condition wait, cancellation, and
# forks while other threads keep malloc and stdio busy (pthread_smoke.c).
t pthread /bin/etc/pthread_smoke
# netfs conversations: a connect to 127.0.0.1 nobody listens on is refused
# at once, and 200 sockets closed right after socket() leak none
# (netconv_smoke.c).
t netconv /bin/etc/netconv_smoke
# netd's loopback interface: getifaddrs and if_nametoindex; UDP over
# 127.0.0.1 (bind, sendto/recvfrom with addresses, EADDRINUSE, connect and
# unconnect, ECONNREFUSED after a datagram to a closed port, the host's
# own address); TCP to a listener on 127.0.0.1; a UDP socket passed by exec
# (bare datagrams to read(), then the socket calls) (loopback_smoke.c).
t loopback /bin/etc/loopback_smoke
# kill(pid, 0); no zombies with SA_NOCLDWAIT or SIGCHLD ignored, ECHILD from
# the wait; setpgid on a child before its exec, EACCES after; a child's
# setsid: its own session, no controlling terminal; SIGINT ends a child
# spinning without syscalls (child_smoke.c).
t child /bin/etc/child_smoke
# getrandom, vfork, daemon, the netdb service lookups and the termios
# constants libgloss gained for the ports, the resolver (localhost without
# a lookup, a name no server knows failing in bounded time), and the soft
# float's double arithmetic, compares, conversions and printf, the long
# double conversions (issue #374); setitimer and alarm (SIGALRM on time, a
# blocking read cut short, the default action) (libc_smoke.c).
t libc /bin/etc/libc_smoke
# The *at calls: a directory fd and the cwd stand for their directory
# whatever is renamed; fstat of an unlinked file; fdopendir; stat follows a
# symlink, lstat does not (at_smoke.c).
t at /bin/etc/at_smoke
# O_EXCL creates a name once, racers or not, and a symlink there is taken;
# ftruncate; pread/pwrite leave the position, ESPIPE on a pipe; close-on-exec
# fds are gone after exec; one read gives a file's bytes up to the count or
# its end; a rename does not wait for a process reading the console; lseek
# and ftello past 2 GiB (fileio_smoke.c; on ext2 in mkfs.ext2's test).
t fileio /bin/etc/fileio_smoke
# MAP_SHARED mappings of a file are the file: stores read back with read()
# and survive munmap, close and a child's exit, write() shows in them, a
# second mapping and a forked child share them, read() lands in them,
# msync, ftruncate, mprotect, a read-only fd; a MAP_PRIVATE mapping is
# copied at its first store (from userspace, the kernel or a forked child),
# and a fault maps the cached pages after it (mmap_smoke.c; on ext2 in
# mkfs.ext2's test).
t mmap /bin/etc/mmap_smoke
# Shared memory: shm_open segments shared by name with a forked child and
# living past shm_unlink for their holders, a memfd shared with a forked
# and an exec'd child, anonymous MAP_SHARED mappings shared with a forked
# child, System V segments by id and by key, no leftover in /dev/shm
# (shm_smoke.c).
t shm /bin/etc/shm_smoke
# A write to a pty whose master is open but never read blocks, and a signal
# ends it: the writer stays killable (fault_smoke.c; a CPU fault from
# userspace is contained the same way, but a test cannot fault on purpose:
# the host treats the kernel's `user fault` line as a failure).
t fault /bin/etc/fault_smoke
# Constructors run before main, by priority, with environ set up;
# destructors at exit, after the atexit handlers (ctor_smoke.c).
t ctor /bin/etc/ctor_smoke
# Idle-pull balancing: a forked child inherits its parent's home CPU and,
# while the parent keeps that CPU busy, an idle CPU takes it: the two end
# up with different homes (sched_smoke.c; nothing to check with one CPU
# for user tasks).
t sched /bin/etc/sched_smoke
# The same for a program tcc builds in the guest: its own linker bounds
# .init_array and .fini_array, and it links crti.o and crtn.o. main returns
# 3, and the destructor turns it into 0 if the constructor ran.
ctor_tcc() {
	cat > /tmp/ctor.c <<'EOF'
#include <unistd.h>
static int ran;
__attribute__((constructor)) static void ctor(void) { ran = 1; }
__attribute__((destructor)) static void dtor(void) { if (ran == 2) _exit(0); }
int main(void) { if (ran == 1) ran = 2; return 3; }
EOF
	/bin/tcc/tcc -o /tmp/ctor /tmp/ctor.c && /tmp/ctor
}
t ctor_tcc ctor_tcc
# /dev/console/kbd: held by one program at a time; the host types Shift+A
# through the QEMU monitor (host.sh sendkey) once the smoke holds the file,
# and the smoke checks the four press and release events (kbd_smoke.c).
kbd_events() {
	: > /tmp/kbd.out
	/bin/etc/kbd_smoke >> /tmp/kbd.out 2>&1 &
	pid=$!
	i=0
	while [ $i -lt 30 ] && ! grep -q -e ready -e "FAIL ]" /tmp/kbd.out 2>/dev/null; do
		sleep 1
		i=$((i + 1))
	done
	grep -q ready /tmp/kbd.out && echo "HOST c-smokes sendkey shift-a" >&3
	# Well inside the launcher's 180 s watchdog: a smoke that hangs fails
	# this test, not the boot.
	i=0
	while [ $i -lt 60 ] && ! grep -q -e "OK ] kbd" -e "FAIL ]" /tmp/kbd.out 2>/dev/null; do
		sleep 1
		i=$((i + 1))
	done
	kill $pid 2>/dev/null
	wait $pid 2>/dev/null
	cat /tmp/kbd.out
	contains "[ OK ] kbd" /tmp/kbd.out
}
t kbd kbd_events
# ^C on the keyboard while nothing reads the console: the console's input
# thread runs the line discipline anyway and the foreground group (this
# shell's) gets SIGINT, so a job that only reads /dev/zero ends. This shell
# ignores it meanwhile; the job is back at the default action.
ctrl_c() {
	trap '' INT
	( trap - INT; exec sha512sum /dev/zero ) &
	pid=$!
	sleep 1
	echo "HOST c-smokes sendkey ctrl-c" >&3
	i=0
	while [ $i -lt 30 ] && kill -0 $pid 2>/dev/null; do
		sleep 1
		i=$((i + 1))
	done
	kill -9 $pid 2>/dev/null
	wait $pid
	rc=$?
	trap - INT
	echo "sha512sum ended with $rc"
	[ $rc -eq 130 ]
}
t ctrl_c ctrl_c
# netd listen/accept: the smoke announces TCP 2323; the host connects back
# through QEMU's port forward (the HOST request runs host.sh tcp-ping),
# sends "ping" and expects "pong", then 280 KB of numbered lines (more than
# netd queues: the smoke's writes wait for room); it connects again to say
# whether every byte arrived, and the smoke reports.
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
