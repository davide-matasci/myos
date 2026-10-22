# The myos sysdeps: C++ shims over the C both of myos's C libraries share
# (libmyos/, copied beside them by toolchain/mlibc/patch.sh), which is
# compiled into libc.a here too, built as mlibc's (MYOS_LIBC_MLIBC: its
# public names are mlibc's, over the sysdeps).
# Not mlibc's Linux option (it wants a Linux kernel's headers): what myos
# answers of it, getrandom, getifaddrs, mount and the mount table, is in
# the shared C, with its headers here (include/).
sysdep_supported_options = {
	'posix': true,
	'glibc': true,
	'bsd': true,
	'linux-epoll': true,
	'linux-timerfd': true,
	'linux-signalfd': true,
	'linux-eventfd': true,
	'linux-reboot': true,
}

add_project_arguments('-DMYOS_LIBC_MLIBC', language: ['c', 'cpp'])
# What g++ defines on its own for C++, and the shared C (written to
# newlib's headers, which declare everything by default) needs for the
# BSD and GNU calls it uses: clang for a bare target defines nothing.
add_project_arguments('-D_GNU_SOURCE', language: ['c', 'cpp'])

libmyos_sources = files(
	'libmyos/myos_raw.c',
	'libmyos/fdio.c',
	'libmyos/proc.c',
	'libmyos/sigtramp.c',
	'libmyos/at.c',
	'libmyos/time.c',
	'libmyos/pwdgrp.c',
	'libmyos/mmap.c',
	'libmyos/shm.c',
	'libmyos/mount.c',
	'libmyos/termios.c',
	'libmyos/ttyctl.c',
	'libmyos/socket.c',
	'libmyos/ifaddrs.c',
	'libmyos/statvfs.c',
	'libmyos/pollselect.c',
	'libmyos/pty.c',
	'libmyos/evfd.c',
	'libmyos/sleep.c',
	'libmyos/uio.c',
	'libmyos/reboot.c',
	'libmyos/ioctl.c',
	'libmyos/mntent.c',
	'libmyos/random.c',
)

# The static ld (the TLS setup of a static program) calls the sysdeps it
# needs from sysdeps.cpp; its raw syscalls are in libmyos/myos_raw.c.
rtld_sources += files(
	'generic/sysdeps.cpp',
	'libmyos/myos_raw.c',
)
rtld_include_dirs += include_directories('libmyos')

libc_sources += files(
	'generic/entry.cpp',
	'generic/sysdeps.cpp',
	'generic/thread.cpp',
	'generic/signal.cpp',
	'generic/dirent.cpp',
	host_machine.cpu_family() / 'thread-entry.S',
)
libc_sources += libmyos_sources
libc_include_dirs += include_directories('libmyos')

if not no_headers
	# The ABI bits: the Linux ones, except where myos differs (its own
	# signal.h, termios.h, ioctls.h and vm-flags.h; the rest are links
	# toolchain/mlibc/patch.sh makes to abis/linux).
	install_headers(
		'include/abi-bits/access.h',
		'include/abi-bits/aio.h',
		'include/abi-bits/auxv.h',
		'include/abi-bits/blkcnt_t.h',
		'include/abi-bits/blksize_t.h',
		'include/abi-bits/clockid_t.h',
		'include/abi-bits/clone-flags.h',
		'include/abi-bits/dev_t.h',
		'include/abi-bits/epoll.h',
		'include/abi-bits/errno.h',
		'include/abi-bits/fcntl.h',
		'include/abi-bits/fd_set.h',
		'include/abi-bits/fsblkcnt_t.h',
		'include/abi-bits/fsfilcnt_t.h',
		'include/abi-bits/gid_t.h',
		'include/abi-bits/in.h',
		'include/abi-bits/ino_t.h',
		'include/abi-bits/inotify.h',
		'include/abi-bits/ioctls.h',
		'include/abi-bits/ipc.h',
		'include/abi-bits/limits.h',
		'include/abi-bits/mode_t.h',
		'include/abi-bits/mqueue.h',
		'include/abi-bits/msg.h',
		'include/abi-bits/nlink_t.h',
		'include/abi-bits/packet.h',
		'include/abi-bits/pid_t.h',
		'include/abi-bits/poll.h',
		'include/abi-bits/ptrace.h',
		'include/abi-bits/random.h',
		'include/abi-bits/reboot.h',
		'include/abi-bits/resource.h',
		'include/abi-bits/rlim_t.h',
		'include/abi-bits/route.h',
		'include/abi-bits/sa_family_t.h',
		'include/abi-bits/sched_param.h',
		'include/abi-bits/seek-whence.h',
		'include/abi-bits/sem.h',
		'include/abi-bits/shm.h',
		'include/abi-bits/sig-limits.h',
		'include/abi-bits/sigevent.h',
		'include/abi-bits/sigset_t.h',
		'include/abi-bits/sigval.h',
		'include/abi-bits/sockaddr_storage.h',
		'include/abi-bits/socket.h',
		'include/abi-bits/socklen_t.h',
		'include/abi-bits/stat.h',
		'include/abi-bits/statfs.h',
		'include/abi-bits/statvfs.h',
		'include/abi-bits/statx.h',
		'include/abi-bits/suseconds_t.h',
		'include/abi-bits/time.h',
		'include/abi-bits/uid_t.h',
		'include/abi-bits/utmp-defines.h',
		'include/abi-bits/utmpx.h',
		'include/abi-bits/utsname.h',
		'include/abi-bits/vm-flags.h',
		'include/abi-bits/vt.h',
		'include/abi-bits/wait.h',
		'include/abi-bits/xattr.h',
		'include/abi-bits/signal.h',
		'include/abi-bits/termios.h',
		subdir: 'abi-bits',
		follow_symlinks: true,
	)
	if host_machine.cpu_family() == 'riscv64'
		install_headers(
			'include/abi-bits/riscv-hwprobe.h',
			subdir: 'abi-bits',
			follow_symlinks: true,
		)
	endif
	# What a program that goes below the libc includes (the raw syscalls,
	# the security calls), as newlib's sysroot carries them.
	install_headers(
		'libmyos/myos_syscalls.h',
		'libmyos/myos_stat.h',
		'libmyos/libmyos.h',
		subdir: 'myos',
	)
	install_headers('include/ifaddrs.h', 'include/mntent.h')
	install_headers(
		'include/sys/myos_extra.h',
		'include/sys/mount.h',
		'include/sys/random.h',
		'include/sys/sysmacros.h',
		subdir: 'sys',
	)
endif

if not headers_only
	crt0 = custom_target('crt0',
		build_by_default: true,
		command: c_compiler.cmd_array() + ['-c', '-o', '@OUTPUT@', '@INPUT@'],
		input: host_machine.cpu_family() / 'crt0.S',
		output: 'crt0.o',
		install: true,
		install_dir: get_option('libdir')
	)
	# The hosted name, for a compiler driver that asks for it (tcc).
	custom_target('crt1',
		build_by_default: true,
		command: c_compiler.cmd_array() + ['-c', '-o', '@OUTPUT@', '@INPUT@'],
		input: host_machine.cpu_family() / 'crt0.S',
		output: 'crt1.o',
		install: true,
		install_dir: get_option('libdir')
	)
	custom_target('crti',
		build_by_default: true,
		command: c_compiler.cmd_array() + ['-c', '-o', '@OUTPUT@', '@INPUT@'],
		input: host_machine.cpu_family() / 'crti.S',
		output: 'crti.o',
		install: true,
		install_dir: get_option('libdir')
	)
	custom_target('crtn',
		build_by_default: true,
		command: c_compiler.cmd_array() + ['-c', '-o', '@OUTPUT@', '@INPUT@'],
		input: host_machine.cpu_family() / 'crtn.S',
		output: 'crtn.o',
		install: true,
		install_dir: get_option('libdir')
	)
endif
