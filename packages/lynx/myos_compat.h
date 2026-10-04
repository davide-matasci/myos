/* Force-included for lynx on myos. */
#ifndef MYOS_LYNX_COMPAT_H
#define MYOS_LYNX_COMPAT_H

#include <sys/types.h>
#include <sys/stat.h>
#include <sys/myos_extra.h>  /* lstat: newlib declares it on Cygwin and RTEMS only */

#ifndef __myos__
#define __myos__ 1
#endif

#endif /* MYOS_LYNX_COMPAT_H */
