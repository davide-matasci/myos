/* myos libgloss: clock_gettime via gettimeofday, settimeofday and
 * clock_settime (the kernel keeps the set time until the next boot; the RTC
 * is not written).
 * Do not define time()/localtime() here — newlib libc already provides them.
 */

#include <errno.h>
#include <stdint.h>
#include <time.h>
#include <sys/time.h>

#include "myos_syscalls.h"

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

int
settimeofday(const struct timeval *tv, const struct timezone *tz)
{
    int64_t raw[2];

    (void)tz;
    if (tv == NULL) {
        return 0; /* only a time zone, which myos does not keep */
    }
    if (tv->tv_usec < 0 || tv->tv_usec >= 1000000 || tv->tv_sec < 0) {
        errno = EINVAL;
        return -1;
    }
    raw[0] = (int64_t)tv->tv_sec;
    raw[1] = (int64_t)tv->tv_usec;
    if (myos_syscall1(MYOS_SYS_SETTIMEOFDAY, (long)(uintptr_t)raw) == (long)MYOS_SYSERR) {
        errno = EINVAL;
        return -1;
    }
    return 0;
}

int
clock_settime(clockid_t clock_id, const struct timespec *tp)
{
    struct timeval tv;

    if (clock_id != CLOCK_REALTIME) {
        errno = EINVAL; /* the monotonic clocks cannot be set */
        return -1;
    }
    if (tp == NULL || tp->tv_nsec < 0 || tp->tv_nsec >= 1000000000L) {
        errno = EINVAL;
        return -1;
    }
    tv.tv_sec = tp->tv_sec;
    tv.tv_usec = tp->tv_nsec / 1000;
    return settimeofday(&tv, NULL);
}
