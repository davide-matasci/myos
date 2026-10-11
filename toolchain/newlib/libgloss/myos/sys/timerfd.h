#ifndef _SYS_TIMERFD_H
#define _SYS_TIMERFD_H

/* timerfd(2) over /dev/timer (docs/events.md, evfd.c): an fd on a kernel
 * timer. A read takes the expirations since the last one (none waits),
 * as 8 bytes; poll says POLLIN while there are some. */
#include <fcntl.h>
#include <time.h>

#define TFD_TIMER_ABSTIME 1
#define TFD_CLOEXEC O_CLOEXEC
#define TFD_NONBLOCK O_NONBLOCK

#ifdef __cplusplus
extern "C" {
#endif

int timerfd_create(clockid_t clockid, int flags);
int timerfd_settime(int fd, int flags, const struct itimerspec *value, struct itimerspec *ovalue);
int timerfd_gettime(int fd, struct itimerspec *value);

#ifdef __cplusplus
}
#endif

#endif /* _SYS_TIMERFD_H */
