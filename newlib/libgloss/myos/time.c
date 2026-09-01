/* myos libgloss: clock_gettime via gettimeofday.
 * Do not define time()/localtime() here — newlib libc already provides them.
 */

#include <time.h>
#include <sys/time.h>

#ifndef CLOCK_REALTIME
#define CLOCK_REALTIME 0
#endif
#ifndef CLOCK_MONOTONIC
#define CLOCK_MONOTONIC 1
#endif

int
clock_gettime(int clock_id, struct timespec *tp)
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
