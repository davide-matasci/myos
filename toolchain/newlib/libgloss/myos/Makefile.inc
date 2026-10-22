# newlib's own part of libgloss. The C both libcs share is in
# toolchain/libmyos, which toolchain/newlib/build-libgloss.sh compiles with
# these into libgloss.a (this file is for a build of libgloss from newlib's
# tree, which myos does not run).
multilibtool_DATA += \
	%D%/myos.specs \
	%D%/crt0.o \
	%D%/crti.o \
	%D%/crtn.o

multilibtool_LIBRARIES += %D%/libgloss.a
%C%_libgloss_a_CPPFLAGS = -I$(srcdir)/%D%
%C%_libgloss_a_SOURCES = \
	%D%/hooks.c \
	%D%/posix_stubs.c \
	%D%/posix_extra.c \
	%D%/misc_stubs.c \
	%D%/more_stubs.c \
	%D%/signal.c \
	%D%/environ.c \
	%D%/initfini.c \
	%D%/getline.c \
	%D%/dirent.c \
	%D%/basename.c \
	%D%/dirname.c \
	%D%/inet.c \
	%D%/netdb.c \
	%D%/search.c \
	%D%/pthread.c \
	%D%/syslog.c

%C%_extra_parts = crt0 crti crtn

%C%_crt0_SOURCES = %D%/crt0.S
