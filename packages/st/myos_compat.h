/* What st expects from libc headers that newlib's lack (-include,
 * build.sh). */
#ifndef MYOS_ST_COMPAT_H
#define MYOS_ST_COMPAT_H

#include <limits.h>
/* openpty: libgloss's, in <pty.h> (st includes it only on Linux). */
#include <pty.h>
#include <time.h>

/* The minimum POSIX guarantees; st sizes its stty command line with it. */
#ifndef _POSIX_ARG_MAX
#define _POSIX_ARG_MAX 4096
#endif

/* libgloss has clock_gettime (time.c); newlib declares it only with
 * _POSIX_TIMERS. */
#ifndef CLOCK_MONOTONIC
#define CLOCK_MONOTONIC 1
#endif
int clock_gettime(clockid_t clock_id, struct timespec *tp);

#endif
