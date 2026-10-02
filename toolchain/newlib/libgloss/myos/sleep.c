/*
 * sleep / usleep / nanosleep for myos: SYS_NANOSLEEP blocks the task in the
 * kernel (its CPU halts) instead of spinning on gettimeofday. Weak so a port
 * that ships its own nanosleep (dropbear's select-based shim) still links.
 */
#include <errno.h>
#include <stddef.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

#include "myos_syscalls.h"

int __myos_sleep_ns(unsigned long long ns, int flags) {
    long ret = myos_syscall2(MYOS_SYS_NANOSLEEP, (long)ns, (long)flags);
    if (ret == (long)MYOS_EINTR) {
        errno = EINTR;
        return -1;
    }
    return 0;
}

__attribute__((weak)) int nanosleep(const struct timespec *req, struct timespec *rem) {
    if (req == NULL || req->tv_nsec < 0 || req->tv_nsec >= 1000000000L || req->tv_sec < 0) {
        errno = EINVAL;
        return -1;
    }
    struct timeval start;
    int have_start = gettimeofday(&start, NULL) == 0;
    unsigned long long ns = (unsigned long long)req->tv_sec * 1000000000ULL
        + (unsigned long long)req->tv_nsec;
    if (__myos_sleep_ns(ns, 0) == 0) {
        if (rem != NULL) {
            rem->tv_sec = 0;
            rem->tv_nsec = 0;
        }
        return 0;
    }
    if (rem != NULL) {
        unsigned long long slept = 0;
        struct timeval now;
        if (have_start && gettimeofday(&now, NULL) == 0) {
            long long us = (long long)(now.tv_sec - start.tv_sec) * 1000000LL
                + (long long)(now.tv_usec - start.tv_usec);
            slept = us > 0 ? (unsigned long long)us * 1000ULL : 0;
        }
        unsigned long long left = slept < ns ? ns - slept : 0;
        rem->tv_sec = (time_t)(left / 1000000000ULL);
        rem->tv_nsec = (long)(left % 1000000000ULL);
    }
    return -1; /* errno = EINTR */
}

__attribute__((weak)) unsigned sleep(unsigned seconds) {
    struct timeval start;
    int have_start = gettimeofday(&start, NULL) == 0;
    if (__myos_sleep_ns((unsigned long long)seconds * 1000000000ULL, 0) == 0) {
        return 0;
    }
    struct timeval now;
    if (have_start && gettimeofday(&now, NULL) == 0) {
        long long s = (long long)(now.tv_sec - start.tv_sec);
        if (s < 0) {
            s = 0;
        }
        return s >= (long long)seconds ? 0 : seconds - (unsigned)s;
    }
    return seconds;
}

__attribute__((weak)) int usleep(useconds_t usec) {
    return __myos_sleep_ns((unsigned long long)usec * 1000ULL, 0);
}
