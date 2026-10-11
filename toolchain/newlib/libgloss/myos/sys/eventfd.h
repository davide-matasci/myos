#ifndef _SYS_EVENTFD_H
#define _SYS_EVENTFD_H

/* eventfd(2) over /dev/event (docs/events.md, evfd.c): an fd on a kernel
 * event counter. A write adds its 8-byte value to the count, a read takes
 * the whole count (zero waits), or one at a time with EFD_SEMAPHORE. */
#include <fcntl.h>
#include <stdint.h>

#define EFD_SEMAPHORE 1
#define EFD_CLOEXEC O_CLOEXEC
#define EFD_NONBLOCK O_NONBLOCK

typedef uint64_t eventfd_t;

#ifdef __cplusplus
extern "C" {
#endif

int eventfd(unsigned int initval, int flags);
int eventfd_read(int fd, eventfd_t *value);
int eventfd_write(int fd, eventfd_t value);

#ifdef __cplusplus
}
#endif

#endif /* _SYS_EVENTFD_H */
