/* What newlib's headers leave out for myos: the scheduling priorities
 * nice and renice name (sys/resource.h has none) and utimensat's special
 * times (sys/stat.h has them for Cygwin and RTEMS only; libgloss's
 * utimensat accepts them and stores nothing). Compile-only for sbase; not copied into the newlib
 * sysroot. */
#ifndef _MYOS_SBASE_COMPAT_H_
#define _MYOS_SBASE_COMPAT_H_

#ifndef UTIME_NOW
#define UTIME_NOW (-2L)
#endif
#ifndef UTIME_OMIT
#define UTIME_OMIT (-1L)
#endif

#ifndef PRIO_PROCESS
#define PRIO_PROCESS 0
#endif
#ifndef PRIO_PGRP
#define PRIO_PGRP 1
#endif
#ifndef PRIO_USER
#define PRIO_USER 2
#endif
#ifndef NZERO
#define NZERO 20
#endif

#endif /* _MYOS_SBASE_COMPAT_H_ */
