/* Extra POSIX limits macros required by os-test limits/ and similar.
 * Installed into the newlib sysroot; included at the end of <limits.h>. */
#ifndef _MYOS_POSIX_LIMITS_H_
#define _MYOS_POSIX_LIMITS_H_

#include <sys/syslimits.h>

/* POSIX minimums (Issue 7) — required by limits/*.c comparisons. */
#ifndef _POSIX_ARG_MAX
# define _POSIX_ARG_MAX 4096
#endif
#ifndef _POSIX_CHILD_MAX
# define _POSIX_CHILD_MAX 25
#endif
#ifndef _POSIX_LINK_MAX
# define _POSIX_LINK_MAX 8
#endif
#ifndef _POSIX_MAX_CANON
# define _POSIX_MAX_CANON 255
#endif
#ifndef _POSIX_MAX_INPUT
# define _POSIX_MAX_INPUT 255
#endif
#ifndef _POSIX_NAME_MAX
# define _POSIX_NAME_MAX 14
#endif
#ifndef _POSIX_NGROUPS_MAX
# define _POSIX_NGROUPS_MAX 8
#endif
#ifndef _POSIX_OPEN_MAX
# define _POSIX_OPEN_MAX 20
#endif
#ifndef _POSIX_PATH_MAX
# define _POSIX_PATH_MAX 256
#endif
#ifndef _POSIX_PIPE_BUF
# define _POSIX_PIPE_BUF 512
#endif
#ifndef _POSIX_SSIZE_MAX
# define _POSIX_SSIZE_MAX 32767
#endif
#ifndef _POSIX_STREAM_MAX
# define _POSIX_STREAM_MAX 8
#endif
#ifndef _POSIX_TZNAME_MAX
# define _POSIX_TZNAME_MAX 6
#endif
#ifndef _XOPEN_IOV_MAX
# define _XOPEN_IOV_MAX 16
#endif

#ifndef SSIZE_MAX
# ifdef __SIZEOF_SIZE_T__
#  if __SIZEOF_SIZE_T__ == 8
#   define SSIZE_MAX 0x7fffffffffffffffL
#  else
#   define SSIZE_MAX 0x7fffffffL
#  endif
# elif defined(LONG_MAX)
#  define SSIZE_MAX LONG_MAX
# else
#  define SSIZE_MAX 0x7fffffffL
# endif
#endif

#ifndef LONG_BIT
# ifdef __SIZEOF_LONG__
#  define LONG_BIT (__SIZEOF_LONG__ * 8)
# else
#  define LONG_BIT 64
# endif
#endif

#ifndef WORD_BIT
# define WORD_BIT 32
#endif

#ifndef _POSIX_HOST_NAME_MAX
# define _POSIX_HOST_NAME_MAX 255
#endif
#ifndef HOST_NAME_MAX
# define HOST_NAME_MAX 255
#endif

#ifndef PAGESIZE
# define PAGESIZE 4096
#endif
#ifndef PAGE_SIZE
# define PAGE_SIZE PAGESIZE
#endif

#endif /* _MYOS_POSIX_LIMITS_H_ */
