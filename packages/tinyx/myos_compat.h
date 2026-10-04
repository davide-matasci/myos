/* What TinyX expects from libc headers that newlib's leave out for myos.
 * Compile-only (-include); not copied into the newlib sysroot. */
#ifndef _MYOS_TINYX_COMPAT_H_
#define _MYOS_TINYX_COMPAT_H_

#include <sys/types.h>
#include <sys/stat.h>
#include <sys/myos_extra.h>   /* lstat: newlib declares it on Cygwin and RTEMS only */

#endif
