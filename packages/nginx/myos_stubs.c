/* Runtime stubs for nginx on myos: only what libgloss has no honest
 * implementation of. */
#include <errno.h>
#include <unistd.h>

#include "myos_compat.h"

/* No limits but the fd table's size (sysconf(_SC_OPEN_MAX)), and no core
 * dumps. */
int getrlimit(int resource, struct rlimit *rlim) {
    if (rlim == NULL) {
        errno = EFAULT;
        return -1;
    }
    switch (resource) {
    case RLIMIT_NOFILE:
        rlim->rlim_cur = rlim->rlim_max = (rlim_t)sysconf(_SC_OPEN_MAX);
        break;
    case RLIMIT_CORE:
        rlim->rlim_cur = rlim->rlim_max = 0;
        break;
    default:
        rlim->rlim_cur = rlim->rlim_max = RLIM_INFINITY;
        break;
    }
    return 0;
}

/* worker_rlimit_nofile and worker_rlimit_core: no more than there is. */
int setrlimit(int resource, const struct rlimit *rlim) {
    struct rlimit have;
    if (getrlimit(resource, &have) != 0) {
        return -1;
    }
    if (rlim->rlim_cur > rlim->rlim_max || rlim->rlim_max > have.rlim_max) {
        errno = EPERM;
        return -1;
    }
    return 0;
}
