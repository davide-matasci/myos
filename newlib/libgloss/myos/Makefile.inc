multilibtool_DATA += \
	%D%/myos.specs \
	%D%/crt0.o

multilibtool_LIBRARIES += %D%/libgloss.a
%C%_libgloss_a_CPPFLAGS = -I$(srcdir)/%D%
%C%_libgloss_a_SOURCES = \
	%D%/myos_raw.c \
	%D%/syscalls.c \
	%D%/stubs.c \
	%D%/posix_stubs.c \
	%D%/environ.c \
	%D%/getline.c

%C%_extra_parts = crt0

%C%_crt0_SOURCES = %D%/crt0.S
