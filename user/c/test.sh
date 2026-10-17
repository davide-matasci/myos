# The C smokes: ptys (openpty round trip through the line discipline, EIO
# once the slave closes) and /dev/urandom (non-zero, distinct, changing).
t -t tty pty /bin/etc/pty_smoke 2
# More pty pairs at once than the kernel once had (four), their names and
# inodes distinct, a closed pair's index used again (pty_smoke.c).
t -t tty ptys /bin/etc/pty_smoke 4
# Writers faster than their reader: a write into a full pty waits for room,
# also on the reader's CPU (pty_smoke.c).
t -t tty pty_flood /bin/etc/pty_smoke 5
# A session's claim on a pair ends with its leader: the next session leader
# with its pid does not hold it, the next claim goes through (pty_smoke.c).
t pty_reclaim /bin/etc/pty_smoke 6
t -t smoke urandom /bin/etc/urandom_smoke
# The tty: the line editor's keys and ^C, driven through a pty (tty_smoke.c).
t -t tty,console tty /bin/etc/tty_smoke
# AF_UNIX over /net/unix: socketpair, listen/accept/connect with a forked
# client, a transfer larger than the buffers, 60000 bytes ahead of the reader,
# 48 conversations at once, EOF, names (unix_smoke.c).
t -t netfs unix /bin/etc/unix_smoke
# /dev/fb: the geometry in ctl, a MAP_SHARED mapping of data that is the
# framebuffer itself (shared with read/write and a forked child), a
# MAP_PRIVATE one that is a copy, and the screen handed to a program and back
# by `text`, the last close of ctl and its holder's exit (fb_smoke.c).
t -t console,fb fb /bin/etc/fb_smoke
# The text console's UTF-8: box-drawing, block and braille characters drawn
# in one cell each, one without a glyph a single '?', a wide one two cells,
# a combining mark none; 256 colors and RGB (fb_smoke.c).
t -t console,fb fb_text /bin/etc/fb_smoke text
# poll: timeouts, waking on pipe and unix socket events (not before),
# POLLHUP/POLLERR/POLLNVAL, EAGAIN, EINTR (poll_smoke.c).
t -t events poll /bin/etc/poll_smoke
# Event counters and timers (docs/events.md): eventfd counts, EAGAIN,
# EINVAL, the semaphore mode, a read woken by a child's write, status; a
# one-shot and a periodic timerfd, gettime, disarm, an absolute time
# (evfd_smoke.c).
t -t events evfd /bin/etc/evfd_smoke
# Wait sets (docs/events.md): epoll over pipes and an event counter, a
# timeout, a child's write ending a wait, EPOLLET, EPOLLONESHOT, the
# errors, a member gone with its fd, a set in a set, the set shared with a
# fork (epoll_smoke.c).
t -t events epoll /bin/etc/epoll_smoke
# Signal fds (docs/events.md): a blocked signal read instead of delivered,
# one outside the mask delivered, EAGAIN, a child's kill ending a read, the
# mask changed, the fd in a wait set (sigfd_smoke.c).
t -t events sigfd /bin/etc/sigfd_smoke
# readv/writev over a pipe, a socketpair and a nonblocking socket that
# fills mid-writev (uio_smoke.c).
t -t fs uio /bin/etc/uio_smoke
# POSIX threads: the API on one thread, then threads contending for a
# mutex and a condition variable, errno, keys, stdio and malloc per thread,
# flockfile, readdir_r, 150 threads started and joined, read-write locks,
# barriers, spin locks, a CLOCK_MONOTONIC condition wait, cancellation, and
# forks while other threads keep malloc and stdio busy (pthread_smoke.c).
t -t sched pthread /bin/etc/pthread_smoke
# netfs conversations: a connect to 127.0.0.1 nobody listens on is refused
# at once, and 200 sockets closed right after socket() leak none
# (netconv_smoke.c).
t -t netfs netconv /bin/etc/netconv_smoke
# netd's loopback interface: getifaddrs and if_nametoindex; UDP over
# 127.0.0.1 (bind, sendto/recvfrom with addresses, EADDRINUSE, connect and
# unconnect, ECONNREFUSED after a datagram to a closed port, the host's
# own address); TCP to a listener on any address (MSG_PEEK) and to one
# bound to 127.0.0.1 (not the host's address, EADDRNOTAVAIL for another's);
# a UDP socket passed by exec (bare datagrams to read(), then the socket
# calls) (loopback_smoke.c).
t -t netfs loopback /bin/etc/loopback_smoke
# kill(pid, 0); no zombies with SA_NOCLDWAIT or SIGCHLD ignored, ECHILD from
# the wait; setpgid on a child before its exec, EACCES after; a child's
# setsid: its own session, no controlling terminal; SIGINT ends a child
# spinning without syscalls; fork shares the pages copy-on-write: each
# side's stores are its own, two forks deep; the shell exec'd from a larger
# image gets an empty heap (child_smoke.c).
t -t sched child /bin/etc/child_smoke
# getrandom, vfork, daemon, the netdb service lookups and the termios
# constants libgloss gained for the ports, the resolver (localhost without
# a lookup, a name no server knows failing in bounded time), and the soft
# float's double arithmetic, compares, conversions and printf, the long
# double conversions (issue #374); setitimer and alarm (SIGALRM on time, a
# blocking read cut short, the default action) (libc_smoke.c).
t -t smoke libc /bin/etc/libc_smoke
# The *at calls: a directory fd and the cwd stand for their directory
# whatever is renamed; fstat of an unlinked file; fdopendir; stat follows a
# symlink, lstat does not (at_smoke.c).
t -t fs at /bin/etc/at_smoke
# O_EXCL creates a name once, racers or not, and a symlink there is taken;
# ftruncate; pread/pwrite leave the position, ESPIPE on a pipe; close-on-exec
# fds are gone after exec; one read gives a file's bytes up to the count or
# its end; a rename does not wait for a process reading the console; lseek
# and ftello past 2 GiB (fileio_smoke.c; on ext2 in mkfs.ext2's test).
t -t fs,smoke fileio /bin/etc/fileio_smoke
# MAP_SHARED mappings of a file are the file: stores read back with read()
# and survive munmap, close and a child's exit, write() shows in them, a
# second mapping and a forked child share them, read() lands in them,
# msync, ftruncate, mprotect, a read-only fd; a MAP_PRIVATE mapping is
# copied at its first store (from userspace, the kernel or a forked child),
# and a fault maps the cached pages after it (mmap_smoke.c; on ext2 in
# mkfs.ext2's test).
t -t mm mmap /bin/etc/mmap_smoke
# Shared memory: shm_open segments shared by name with a forked child and
# living past shm_unlink for their holders, a memfd shared with a forked
# and an exec'd child, anonymous MAP_SHARED mappings shared with a forked
# child, System V segments by id and by key, no leftover in /dev/shm
# (shm_smoke.c).
t -t mm,fs shm /bin/etc/shm_smoke
# A write to a pty whose master is open but never read blocks, and a signal
# ends it: the writer stays killable (fault_smoke.c; a CPU fault from
# userspace is contained the same way, but a test cannot fault on purpose:
# the host treats the kernel's `user fault` line as a failure).
t -t mm fault /bin/etc/fault_smoke
# A JIT's pattern from two threads: one makes a range of pages not touched
# yet executable with mprotect while the other reads them, then code
# written into every page runs. A page the reader faulted in while mprotect
# walked the range used to keep the old protection, and its first fetch
# killed the process (jit_smoke.c, issue #492).
t -t mm,sched jit /bin/etc/jit_smoke
# Constructors run before main, by priority, with environ set up;
# destructors at exit, after the atexit handlers (ctor_smoke.c).
t -t exec ctor /bin/etc/ctor_smoke
# Idle-pull balancing: a forked child inherits its parent's home CPU and,
# while the parent keeps that CPU busy, an idle CPU takes it: the two end
# up with different homes (sched_smoke.c; nothing to check with one CPU
# for user tasks).
t -t sched sched /bin/etc/sched_smoke
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
t -t exec ctor_tcc ctor_tcc
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
t -t console kbd kbd_events
# A burst of keys while /dev/console/kbd is held: the host types `a` 200
# times, 10 ms apart, and each press and release arrives once, in order
# (400 lines, no line twice in a row). The console's input thread and the
# reader drain the keyboard together; the PS/2 controller once gave both
# the same byte, a second press.
kbd_burst() {
	: > /tmp/kb.out
	cat /dev/console/kbd > /tmp/kb.out &
	pid=$!
	sleep 1
	echo "HOST c-smokes keyburst 200 a" >&3
	i=0
	while [ $i -lt 30 ] && [ "$(wc -l < /tmp/kb.out)" -lt 400 ]; do
		sleep 1
		i=$((i + 1))
	done
	sleep 1
	kill $pid 2>/dev/null
	wait $pid 2>/dev/null
	lines=$(wc -l < /tmp/kb.out)
	dups=$(uniq -d /tmp/kb.out | wc -l)
	echo "$lines events, $dups repeated"
	[ "$lines" -eq 400 ] && [ "$dups" -eq 0 ]
}
t -t console kbd_burst kbd_burst
# /dev/console/mouse: held by one program at a time; the host moves the
# usb-mouse and clicks through the QEMU monitor (host.sh mouse) once the
# smoke holds the file, and the smoke checks the motion and the click
# (mouse_smoke.c). /proc/usb names the driver and what it made of it.
# mouse_smoke_run REQUEST: the smoke, with `HOST c-smokes REQUEST` sent
# once it is ready.
mouse_smoke_run() {
	: > /tmp/mouse.out
	/bin/etc/mouse_smoke >> /tmp/mouse.out 2>&1 &
	pid=$!
	i=0
	while [ $i -lt 30 ] && ! grep -q -e ready -e "FAIL ]" /tmp/mouse.out 2>/dev/null; do
		sleep 1
		i=$((i + 1))
	done
	grep -q ready /tmp/mouse.out && echo "HOST c-smokes $1" >&3
	i=0
	while [ $i -lt 60 ] && ! grep -q -e "OK ] mouse" -e "FAIL ]" /tmp/mouse.out 2>/dev/null; do
		sleep 1
		i=$((i + 1))
	done
	kill $pid 2>/dev/null
	wait $pid 2>/dev/null
	cat /tmp/mouse.out
	contains "[ OK ] mouse" /tmp/mouse.out
}
mouse_events() {
	grep -q ':usb_hid mouse$' /proc/usb || { cat /proc/usb; return 1; }
	mouse_smoke_run mouse
}
t -t console,usb,mouse mouse mouse_events
# The console's own mice feed the same file: on x86_64 the PS/2 mouse (the
# keyboard's controller, its aux port on IRQ 12), on the virt boards a
# virtio-input mouse beside the virtio keyboard. The host moves and clicks
# that mouse instead (host.sh mouse ps2|virtio), and its interrupt's count
# says the events came that way, not from the usb-mouse.
# mouse_irqs NAME: the count of interrupt NAME in /proc/interrupts.
mouse_irqs() {
	grep " $1\$" /proc/interrupts | while read -r n count rest; do echo "$count"; done
}
# mouse_via IRQ REQUEST: the smoke through the mouse whose interrupt is IRQ.
mouse_via() {
	before=$(mouse_irqs "$1")
	[ -n "$before" ] || { cat /proc/interrupts; return 1; }
	mouse_smoke_run "$2" || return 1
	after=$(mouse_irqs "$1")
	echo "$1: $before, then $after"
	[ "$after" -gt "$before" ]
}
ps2_mouse_events() {
	grep -q ' ps2 keyboard$' /proc/interrupts || return 0
	mouse_via "ps2 mouse" "mouse ps2"
}
t -t console,mouse ps2_mouse ps2_mouse_events
virtio_mouse_events() {
	grep -q ' virtio-input$' /proc/interrupts || return 0
	mouse_via virtio-pointer "mouse virtio"
}
t -t console,mouse virtio_mouse virtio_mouse_events
# The keyboard types at the tty whose text screen is shown (docs/tty.md):
# tty9's here, which has no getty. ^C there while nothing reads it: the
# console's input thread runs the line discipline anyway and the tty's
# session (kbd_smoke intr, which reads only /dev/zero) gets SIGINT.
ctrl_c() {
	echo 'screen 9' > /dev/console/ctl || return 1
	/bin/etc/kbd_smoke intr /dev/tty9/data &
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
	echo 'screen 1' > /dev/console/ctl
	echo "kbd_smoke intr ended with $rc"
	[ $rc -eq 130 ]
}
t -t tty,console ctrl_c ctrl_c
# Caps Lock on the keyboard: letters take the other case, Shift with it
# gives lowercase, digits stay. QEMU's VNC server types an uppercase letter
# sent without Shift (a pasted one) as Caps Lock around it. The line is read
# from tty9, its screen shown, by a job that is killed after 30 s. Alt+1
# then shows tty1's screen again.
caps_lock() {
	: > /tmp/caps.out
	echo 'screen 9' > /dev/console/ctl || return 1
	head -n 1 < /dev/tty9/data > /tmp/caps.out &
	pid=$!
	echo "HOST c-smokes sendkey caps_lock a 1 shift-b caps_lock c ret alt-1" >&3
	i=0
	while [ $i -lt 30 ] && kill -0 $pid 2>/dev/null; do
		sleep 1
		i=$((i + 1))
	done
	kill $pid 2>/dev/null
	wait $pid 2>/dev/null
	echo "read: $(cat /tmp/caps.out)"
	[ "$(cat /tmp/caps.out)" = "A1bc" ] || return 1
	i=0
	while [ $i -lt 10 ] && ! grep -q '^screen 1$' /dev/console/ctl; do
		sleep 1
		i=$((i + 1))
	done
	grep '^screen' /dev/console/ctl
	grep -q '^screen 1$' /dev/console/ctl
}
t -t console caps_lock caps_lock
# A USB keyboard (usb_hid's boot keyboard, into the console's keyboard
# service): the host plugs a usb-kbd in and types on it (host.sh usb-kbd).
# Shift+A reaches /dev/console/kbd as four events; then tty9 reads a line
# typed with Caps Lock (whose LED usb_hid sets) and an `x` held for a
# second, which the console repeats (a USB keyboard does not); Alt+1 shows
# tty1 again. The console's own keyboard took none of it (its interrupt
# count), and the keyboard is gone from /proc/usb once pulled out.
usb_keyboard() {
	native=$(grep -o -e ' ps2 keyboard$' -e ' virtio-input$' /proc/interrupts | head -n 1 | sed 's/^ //')
	before=$(mouse_irqs "$native")
	echo "HOST c-smokes usb-kbd plug" >&3
	i=0
	while [ $i -lt 30 ] && ! grep -q ':usb_hid keyboard$' /proc/usb; do
		sleep 1
		i=$((i + 1))
	done
	ok=0
	grep -q ':usb_hid keyboard$' /proc/usb || { cat /proc/usb; ok=1; }
	: > /tmp/ukbd.out
	cat /dev/console/kbd > /tmp/ukbd.out &
	pid=$!
	sleep 1
	echo "HOST c-smokes sendkey shift-a" >&3
	i=0
	while [ $i -lt 30 ] && [ "$(wc -l < /tmp/ukbd.out)" -lt 4 ]; do
		sleep 1
		i=$((i + 1))
	done
	sleep 1
	kill $pid 2>/dev/null
	wait $pid 2>/dev/null
	for e in 'd 42' 'd 30 A' 'u 30' 'u 42'; do
		grep -qx "$e" /tmp/ukbd.out || { echo "no \"$e\" in:"; cat /tmp/ukbd.out; ok=1; }
	done
	[ "$(wc -l < /tmp/ukbd.out)" -eq 4 ] || { echo "events:"; cat /tmp/ukbd.out; ok=1; }
	: > /tmp/ukbd.line
	echo 'screen 9' > /dev/console/ctl || return 1
	head -n 1 < /dev/tty9/data > /tmp/ukbd.line &
	pid=$!
	echo "HOST c-smokes sendkey caps_lock a shift-b caps_lock c x@1000 ret alt-1" >&3
	i=0
	while [ $i -lt 30 ] && kill -0 $pid 2>/dev/null; do
		sleep 1
		i=$((i + 1))
	done
	kill $pid 2>/dev/null
	wait $pid 2>/dev/null
	line=$(cat /tmp/ukbd.line)
	echo "read: $line"
	# The held x: its press and at least four repeats, not one every tick.
	echo "$line" | grep -qx 'Abcx\{5,40\}' || ok=1
	i=0
	while [ $i -lt 10 ] && ! grep -q '^screen 1$' /dev/console/ctl; do
		sleep 1
		i=$((i + 1))
	done
	grep -q '^screen 1$' /dev/console/ctl || { echo 'screen 1' > /dev/console/ctl; echo "no Alt+1"; ok=1; }
	after=$(mouse_irqs "$native")
	[ "$after" = "$before" ] || { echo "$native: $before, then $after"; ok=1; }
	echo "HOST c-smokes usb-kbd unplug" >&3
	i=0
	while [ $i -lt 30 ] && grep -q ':usb_hid keyboard$' /proc/usb; do
		sleep 1
		i=$((i + 1))
	done
	! grep -q ':usb_hid keyboard$' /proc/usb || { cat /proc/usb; ok=1; }
	return $ok
}
t -t console,usb,keyboard usb_keyboard usb_keyboard
# netd listen/accept: the smoke announces TCP 2323; the host connects back
# through QEMU's port forward (the HOST request runs host.sh tcp-ping),
# sends "ping" and expects "pong", then 280 KB of numbered lines (more than
# netd queues: the smoke's writes wait for room); it connects again to say
# whether every byte arrived, then 40 times without a word (the FIN at
# varying moments after the handshake), each of which the smoke must see
# end, and the smoke reports. (Before it listens for the host, the smoke
# checks that a second listener on its port is refused.)
net_listen() {
	: > /tmp/listen.out
	/bin/etc/tcp_listen_smoke >> /tmp/listen.out 2>&1 &
	pid=$!
	echo "HOST c-smokes tcp-ping 2323" >&3
	# netd's table (/net/convs) names the listener while it is up.
	i=0
	while [ $i -lt 30 ] && ! grep -q 'listen \*!2323' /net/convs 2>/dev/null; do
		sleep 1
		i=$((i + 1))
	done
	grep -q 'listen \*!2323' /net/convs || { echo "no listener on 2323 in /net/convs:"; cat /net/convs; }
	i=0
	while [ $i -lt 180 ] && ! grep -q -e "OK ] listen" -e "FAIL ]" /tmp/listen.out 2>/dev/null; do
		sleep 1
		i=$((i + 1))
	done
	kill $pid 2>/dev/null
	wait $pid 2>/dev/null
	cat /tmp/listen.out
	contains "[ OK ] listen" /tmp/listen.out && ! grep -q "no listener on 2323" /tmp/listen.out
}
t -t netfs listen net_listen
