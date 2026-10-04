/* Extra declarations for freestanding Vim on myos.
 * Compile-only via -include; not copied into the newlib sysroot. */
#ifndef _MYOS_VIM_COMPAT_H_
#define _MYOS_VIM_COMPAT_H_

#include <sys/types.h>
#include <sys/wait.h>
#include <sys/myos_extra.h>  /* lstat: newlib declares it on Cygwin and RTEMS only */
#include <unistd.h>

/* No core dumps on myos; newlib's sys/wait.h has no WCOREDUMP. */
#ifndef WCOREDUMP
#define WCOREDUMP(s) 0
#endif

#endif /* _MYOS_VIM_COMPAT_H_ */
