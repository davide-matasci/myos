/* clock_gettime and clock_getres (CLOCK_REALTIME via
 * gettimeofday, the monotonic clocks the kernel's time since boot, which
 * settimeofday does not move), settimeofday and clock_settime (the kernel
 * keeps the set time until the next boot; the RTC is not written).
 * Do not define time()/localtime() here — newlib libc already provides them.
 */

#define _GNU_SOURCE /* CLOCK_BOOTTIME and the _RAW and _COARSE clocks */
#include <errno.h>
#include <stdint.h>
#include <time.h>
#include <sys/time.h>

#include "myos_syscalls.h"
#include "libc.h"

/* <time.h> declares clock_gettime and the CLOCK_* ids: the installed
 * features.h sets _POSIX_TIMERS (toolchain/newlib/build-libgloss.sh). */

/* CLOCK_MONOTONIC and the GNU clocks on the same count: myos does not
 * suspend (CLOCK_BOOTTIME is the same time) nor slew it (_RAW), and reads
 * it cheaply enough (_COARSE). */
static int
is_monotonic(clockid_t clock_id)
{
    return clock_id == CLOCK_MONOTONIC || clock_id == CLOCK_MONOTONIC_RAW
        || clock_id == CLOCK_MONOTONIC_COARSE || clock_id == CLOCK_BOOTTIME;
}

static int
is_realtime(clockid_t clock_id)
{
    return clock_id == CLOCK_REALTIME || clock_id == CLOCK_REALTIME_COARSE;
}

int
__myos_clock_gettime(clockid_t clock_id, struct timespec *tp)
{
    struct timeval tv;

    if (tp == NULL || !(is_monotonic(clock_id) || is_realtime(clock_id))) {
        errno = EINVAL;
        return -1;
    }
    if (is_monotonic(clock_id)) {
        unsigned long ns = (unsigned long)myos_syscall0(MYOS_SYS_CLOCK_MONOTONIC);
        tp->tv_sec = (time_t)(ns / 1000000000UL);
        tp->tv_nsec = (long)(ns % 1000000000UL);
        return 0;
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

/* The monotonic clocks count nanoseconds, the wall clock microseconds. */
int
__myos_clock_getres(clockid_t clock_id, struct timespec *res)
{
    if (!(is_monotonic(clock_id) || is_realtime(clock_id))) {
        errno = EINVAL;
        return -1;
    }
    if (res != NULL) {
        res->tv_sec = 0;
        res->tv_nsec = is_monotonic(clock_id) ? 1L : 1000L;
    }
    return 0;
}

int
__myos_settimeofday(const struct timeval *tv, const struct timezone *tz)
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
__myos_clock_settime(clockid_t clock_id, const struct timespec *tp)
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
    return __myos_settimeofday(&tv, NULL);
}

MYOS_ALIAS(clock_gettime)
MYOS_ALIAS(clock_getres)
MYOS_ALIAS(settimeofday)
MYOS_ALIAS(clock_settime)
