/* Extra POSIX/BSD declarations exposed by host libc but gated in newlib. */
#ifndef _MYOS_SBASE_COMPAT_H_
#define _MYOS_SBASE_COMPAT_H_

#include <stdio.h>
#include <sys/types.h>
#include <limits.h>

#ifndef SSIZE_MAX
#ifdef LONG_MAX
#define SSIZE_MAX ((ssize_t)(LONG_MAX))
#else
#define SSIZE_MAX ((ssize_t)9223372036854775807LL)
#endif
#endif

#ifndef PRIO_USER
#define PRIO_USER 1
#endif

#ifndef UTIME_OMIT
#define UTIME_OMIT (-1)
#endif

#ifndef UTIME_NOW
#define UTIME_NOW (-2)
#endif

#ifndef _POSIX_ARG_MAX
#define _POSIX_ARG_MAX 4096
#endif

#ifndef FNM_CASEFOLD
#define FNM_CASEFOLD (1 << 4)
#endif

#ifndef FNM_LEADING_DIR
#define FNM_LEADING_DIR (1 << 5)
#endif

#ifndef _POSIX_HOST_NAME_MAX
#define _POSIX_HOST_NAME_MAX 255
#endif

#ifndef _POSIX_PATH_MAX
#define _POSIX_PATH_MAX 4096
#endif

#ifndef _POSIX_NAME_MAX
#define _POSIX_NAME_MAX 255
#endif

#ifndef PRIO_PGRP
#define PRIO_PGRP 1
#endif

#ifndef NZERO
#define NZERO 20
#endif

#ifndef PRIO_PROCESS
#define PRIO_PROCESS 0
#endif

ssize_t getline(char **lineptr, size_t *n, FILE *stream);
ssize_t getdelim(char **lineptr, size_t *n, int delim, FILE *stream);

#endif /* _MYOS_SBASE_COMPAT_H_ */
