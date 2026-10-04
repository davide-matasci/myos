/* myos libgloss: clock_gettime via gettimeofday.
 * Do not define time()/localtime() here — newlib libc already provides them.
 */

#include <time.h>
#include <sys/time.h>

/* <time.h> declares clock_gettime and the CLOCK_* ids: the installed
 * features.h sets _POSIX_TIMERS (toolchain/newlib/build-libgloss.sh). */

int
clock_gettime(clockid_t clock_id, struct timespec *tp)
{
    struct timeval tv;

    (void)clock_id;
    if (tp == NULL) {
        return -1;
    }
    if (gettimeofday(&tv, NULL) != 0) {
        tp->tv_sec = 0;
        tp->tv_nsec = 0;
        return -1;
    }
    tp->tv_sec = (time_t)tv.tv_sec;
    tp->tv_nsec = (long)tv.tv_usec * 1000L;
    return 0;
}
