# The C smokes once more, their mlibc builds (/bin/etc/mlibc, port.env):
# every test of user/c/test.sh (the file sourced before this one: its
# functions run here too, on $SMOKES) as mlibc_<name>, tagged mlibc and
# <name> (a selection or a skip by the newlib test's name takes both:
# the Raspberry Pi 3 boot skips `fileio` and `libc`, docs/testing.md), but
# the ones that run no smoke program (kbd_burst, caps_lock, usb_keyboard). What each one checks
# is said there; a difference between the two libcs' results is mlibc's
# sysdeps' (toolchain/mlibc/sysdeps/myos) or the shared C's
# (toolchain/libmyos), docs/mlibc.md.
MLIBC=/bin/etc/mlibc
# mlibc_run COMMAND...: a function of user/c/test.sh on the mlibc smokes.
mlibc_run() {
	SMOKES=$MLIBC
	"$@"
	mlibc_rc=$?
	SMOKES=/bin/etc
	return $mlibc_rc
}
t -t smoke,mlibc,hello mlibc_hello $MLIBC/hello
t -t tty,mlibc,pty mlibc_pty $MLIBC/pty_smoke 2
t -t tty,mlibc,ptys mlibc_ptys $MLIBC/pty_smoke 4
t -t tty,mlibc,pty_flood mlibc_pty_flood $MLIBC/pty_smoke 5
t -t mlibc,pty_reclaim mlibc_pty_reclaim $MLIBC/pty_smoke 6
t -t smoke,mlibc,urandom mlibc_urandom $MLIBC/urandom_smoke
t -t tty,console,mlibc mlibc_tty $MLIBC/tty_smoke
t -t netfs,mlibc,unix mlibc_unix $MLIBC/unix_smoke
t -t netfs,mlibc,unix,unix_seqpacket mlibc_unix_seqpacket $MLIBC/unix_smoke seqpacket
t -t console,fb,mlibc mlibc_fb $MLIBC/fb_smoke
t -t console,fb,mlibc,fb_text mlibc_fb_text $MLIBC/fb_smoke text
t -t events,mlibc,poll mlibc_poll $MLIBC/poll_smoke
t -t events,mlibc,evfd mlibc_evfd $MLIBC/evfd_smoke
t -t events,mlibc,epoll mlibc_epoll $MLIBC/epoll_smoke
t -t events,mlibc,sigfd mlibc_sigfd $MLIBC/sigfd_smoke
t -t signal,mlibc,sigspin mlibc_sigspin $MLIBC/sigspin_smoke
t -t pcidev,mlibc mlibc_pcidev mlibc_run pcidev_test
t -t tap,mlibc mlibc_tap $MLIBC/tap_smoke
t -t tap,netfs,mlibc,multihome mlibc_multihome $MLIBC/multihome_smoke
t -t tap,netfs,mlibc,multihome_clock mlibc_multihome_clock mlibc_run multihome_clock
t -t fs,mlibc,uio mlibc_uio $MLIBC/uio_smoke
t -t sched,mlibc,pthread mlibc_pthread $MLIBC/pthread_smoke
t -t netfs,mlibc,netconv mlibc_netconv $MLIBC/netconv_smoke
t -t netfs,mlibc,ipv6 mlibc_ipv6 $MLIBC/ipv6_smoke
t -t net,netfs,mlibc,ipv6_dns mlibc_ipv6_dns $MLIBC/ipv6_smoke dns
t -t netfs,mlibc,loopback mlibc_loopback $MLIBC/loopback_smoke
t -t sched,mlibc,child mlibc_child $MLIBC/child_smoke
t -t smoke,mlibc,libc mlibc_libc $MLIBC/libc_smoke
t -t fs,mlibc,at mlibc_at $MLIBC/at_smoke
t -t fs,smoke,mlibc,fileio mlibc_fileio $MLIBC/fileio_smoke
t -t mm,mlibc,mmap mlibc_mmap $MLIBC/mmap_smoke
t -t mm,exec,mlibc,bigimage mlibc_bigimage mlibc_run bigimage_run
t -t mm,fs,mlibc,shm mlibc_shm $MLIBC/shm_smoke
t -t mm,mlibc,fault mlibc_fault $MLIBC/fault_smoke
t -t mm,sched,mlibc,jit mlibc_jit $MLIBC/jit_smoke
t -t exec,mlibc,ctor mlibc_ctor $MLIBC/ctor_smoke
t -t sched,mlibc mlibc_sched $MLIBC/sched_smoke
t -t console,mlibc,kbd mlibc_kbd mlibc_run kbd_events
t -t console,usb,mouse,mlibc mlibc_mouse mlibc_run mouse_events
t -t console,mouse,mlibc,ps2_mouse mlibc_ps2_mouse mlibc_run ps2_mouse_events
t -t console,mouse,mlibc,virtio_mouse mlibc_virtio_mouse mlibc_run virtio_mouse_events
t -t tty,console,mlibc,ctrl_c mlibc_ctrl_c mlibc_run ctrl_c
t -t netfs,mlibc,listen mlibc_listen mlibc_run net_listen
