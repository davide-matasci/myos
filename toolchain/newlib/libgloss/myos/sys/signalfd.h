#ifndef _SYS_SIGNALFD_H
#define _SYS_SIGNALFD_H

/* signalfd(2) over /dev/sig (docs/events.md, evfd.c): an fd on a signal
 * mask. A read takes the reading process's pending signals of the mask
 * (block them first, else they are delivered instead), one record each;
 * poll says POLLIN while one is pending. The record is myos's: the first
 * fields of Linux's struct signalfd_siginfo, 16 bytes, the sender fields
 * zero (the kernel keeps no sender). A program that reads
 * sizeof(struct signalfd_siginfo) records and looks at ssi_signo works
 * as on Linux. */
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>

#define SFD_CLOEXEC O_CLOEXEC
#define SFD_NONBLOCK O_NONBLOCK

struct signalfd_siginfo {
    uint32_t ssi_signo;
    int32_t ssi_code;
    uint32_t ssi_pid;
    uint32_t ssi_uid;
};

#ifdef __cplusplus
extern "C" {
#endif

int signalfd(int fd, const sigset_t *mask, int flags);

#ifdef __cplusplus
}
#endif

#endif /* _SYS_SIGNALFD_H */
