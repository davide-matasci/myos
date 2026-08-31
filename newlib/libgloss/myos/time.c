/* myos libgloss: clock_gettime from _gettimeofday (libc owns time/localtime). */

#include <errno.h>
#include <time.h>
#include <sys/time.h>
#include <sys/types.h>

int
clock_gettime(clockid_t clock_id, struct timespec *tp)
{
    struct timeval tv;

    (void)clock_id;
    if (tp == NULL) {
        errno = EINVAL;
        return -1;
    }
    if (gettimeofday(&tv, NULL) != 0) {
        tp->tv_sec = 0;
        tp->tv_nsec = 0;
        return 0;
    }
    tp->tv_sec = (time_t)tv.tv_sec;
    tp->tv_nsec = (long)tv.tv_usec * 1000L;
    return 0;
}
