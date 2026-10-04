/* What dmenu and stest expect from libc headers that newlib's lack
 * (-include, build.sh): libgloss has both functions. */
#ifndef MYOS_DMENU_COMPAT_H
#define MYOS_DMENU_COMPAT_H

#include <stdio.h>
#include <time.h>

ssize_t getline(char **lineptr, size_t *n, FILE *stream);
/* newlib declares it only with _POSIX_TIMERS. */
int nanosleep(const struct timespec *req, struct timespec *rem);

#endif
