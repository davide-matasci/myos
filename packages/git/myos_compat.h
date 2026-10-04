/* Extra declarations for freestanding Git on myos.
 * Compile-only via -include; not copied into the newlib sysroot. */
#ifndef _MYOS_GIT_COMPAT_H_
#define _MYOS_GIT_COMPAT_H_

#include <sys/types.h>
#include <sys/myos_extra.h>  /* lstat: newlib declares it on Cygwin and RTEMS only */
#include <sys/wait.h>
#include <signal.h>
#include <unistd.h>
#include <limits.h>
#include <stdio.h>

/* newlib declares these only under _GNU_SOURCE, which git's build does not
 * set; libgloss implements them (getline.c). */
ssize_t getdelim(char **lineptr, size_t *n, int delim, FILE *stream);
ssize_t getline(char **lineptr, size_t *n, FILE *stream);

/* exec-cmd.c sizes a buffer with it; newlib keeps it in <sys/param.h>, which
 * git does not include. */
#ifndef MAXPATHLEN
#define MAXPATHLEN 1024
#endif

/* No core dumps on myos; newlib's sys/wait.h has no WCOREDUMP. */
#ifndef WCOREDUMP
#define WCOREDUMP(s) 0
#endif

#endif /* _MYOS_GIT_COMPAT_H_ */
