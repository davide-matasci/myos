# The C smokes once more, their mlibc builds (/bin/etc/mlibc, port.env):
# every test of user/c/test.sh (the file sourced before this one: its
# functions run here too, on $SMOKES) as mlibc_<name>, tagged mlibc, but
# ctor_tcc (its program tcc links against newlib) and the ones that run no
# smoke program (kbd_burst, caps_lock, usb_keyboard). What each one checks
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
t -t smoke,mlibc mlibc_hello $MLIBC/hello
t -t tty,mlibc mlibc_pty $MLIBC/pty_smoke 2
t -t tty,mlibc mlibc_ptys $MLIBC/pty_smoke 4
t -t tty,mlibc mlibc_pty_flood $MLIBC/pty_smoke 5
t -t mlibc mlibc_pty_reclaim $MLIBC/pty_smoke 6
t -t smoke,mlibc mlibc_urandom $MLIBC/urandom_smoke
t -t tty,console,mlibc mlibc_tty $MLIBC/tty_smoke
t -t netfs,mlibc mlibc_unix $MLIBC/unix_smoke
t -t console,fb,mlibc mlibc_fb $MLIBC/fb_smoke
t -t console,fb,mlibc mlibc_fb_text $MLIBC/fb_smoke text
t -t events,mlibc mlibc_poll $MLIBC/poll_smoke
t -t events,mlibc mlibc_evfd $MLIBC/evfd_smoke
t -t events,mlibc mlibc_epoll $MLIBC/epoll_smoke
t -t events,mlibc mlibc_sigfd $MLIBC/sigfd_smoke
t -t pcidev,mlibc mlibc_pcidev mlibc_run pcidev_test
t -t tap,mlibc mlibc_tap $MLIBC/tap_smoke
t -t tap,netfs,mlibc mlibc_multihome $MLIBC/multihome_smoke
t -t tap,netfs,mlibc mlibc_multihome_clock mlibc_run multihome_clock
t -t fs,mlibc mlibc_uio $MLIBC/uio_smoke
t -t sched,mlibc mlibc_pthread $MLIBC/pthread_smoke
t -t netfs,mlibc mlibc_netconv $MLIBC/netconv_smoke
t -t netfs,mlibc mlibc_loopback $MLIBC/loopback_smoke
t -t sched,mlibc mlibc_child $MLIBC/child_smoke
t -t smoke,mlibc mlibc_libc $MLIBC/libc_smoke
t -t fs,mlibc mlibc_at $MLIBC/at_smoke
t -t fs,smoke,mlibc mlibc_fileio $MLIBC/fileio_smoke
t -t mm,mlibc mlibc_mmap $MLIBC/mmap_smoke
t -t mm,fs,mlibc mlibc_shm $MLIBC/shm_smoke
t -t mm,mlibc mlibc_fault $MLIBC/fault_smoke
t -t mm,sched,mlibc mlibc_jit $MLIBC/jit_smoke
t -t exec,mlibc mlibc_ctor $MLIBC/ctor_smoke
t -t sched,mlibc mlibc_sched $MLIBC/sched_smoke
t -t console,mlibc mlibc_kbd mlibc_run kbd_events
t -t console,usb,mouse,mlibc mlibc_mouse mlibc_run mouse_events
t -t console,mouse,mlibc mlibc_ps2_mouse mlibc_run ps2_mouse_events
t -t console,mouse,mlibc mlibc_virtio_mouse mlibc_run virtio_mouse_events
t -t tty,console,mlibc mlibc_ctrl_c mlibc_run ctrl_c
t -t netfs,mlibc mlibc_listen mlibc_run net_listen
