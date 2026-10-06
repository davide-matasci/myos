multilibtool_DATA += \
	%D%/myos.specs \
	%D%/crt0.o \
	%D%/crti.o \
	%D%/crtn.o

multilibtool_LIBRARIES += %D%/libgloss.a
%C%_libgloss_a_CPPFLAGS = -I$(srcdir)/%D%
%C%_libgloss_a_SOURCES = \
	%D%/myos_raw.c \
	%D%/syscalls.c \
	%D%/stubs.c \
	%D%/posix_stubs.c \
	%D%/misc_stubs.c \
	%D%/signal.c \
	%D%/more_stubs.c \
	%D%/ioctl.c \
	%D%/environ.c \
	%D%/getline.c \
	%D%/mmap.c \
	%D%/at.c \
	%D%/termios.c \
	%D%/ttyctl.c \
	%D%/socket.c \
	%D%/inet.c \
	%D%/netdb.c \
	%D%/pollselect.c \
	%D%/pty.c \
	%D%/sleep.c \
	%D%/uio.c \
	%D%/pthread.c

%C%_extra_parts = crt0 crti crtn

%C%_crt0_SOURCES = %D%/crt0.S
