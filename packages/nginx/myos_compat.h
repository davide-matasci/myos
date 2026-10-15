/* Force-included for nginx on myos: what newlib's headers lack. */
#ifndef MYOS_NGINX_COMPAT_H
#define MYOS_NGINX_COMPAT_H

#include <sys/types.h>
#include <sys/time.h>
#include <time.h>
#include <sys/myos_extra.h>  /* lstat: newlib declares it on Cygwin and RTEMS only */

/* POSIX's timezone variable, newlib's _timezone (ngx_time.h's
 * ngx_timezone: struct tm has no tm_gmtoff). Defined after <sys/time.h>,
 * whose struct timezone keeps its name. */
#define timezone _timezone

/* myos has no resource limits: getrlimit reports what there is
 * (myos_stubs.c), setrlimit accepts no more than that. */
#define RLIMIT_CORE 4
#define RLIMIT_NOFILE 7
#define RLIM_INFINITY ((rlim_t)-1)
typedef unsigned long rlim_t;
struct rlimit {
    rlim_t rlim_cur;
    rlim_t rlim_max;
};
int getrlimit(int resource, struct rlimit *rlim);
int setrlimit(int resource, const struct rlimit *rlim);

/* worker_priority: libgloss has setpriority (ENOSYS: no priorities),
 * newlib's <sys/resource.h> no declaration. */
#define PRIO_PROCESS 0
int setpriority(int which, id_t who, int prio);

#endif /* MYOS_NGINX_COMPAT_H */
