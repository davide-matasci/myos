#ifndef _MYOS_POLL_H_
#define _MYOS_POLL_H_

#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef unsigned long nfds_t;

#define POLLIN   0x0001
#define POLLPRI  0x0002
#define POLLOUT  0x0004
#define POLLERR  0x0008
#define POLLHUP  0x0010
#define POLLNVAL 0x0020
#define POLLRDNORM POLLIN
#define POLLWRNORM POLLOUT

struct pollfd {
    int fd;
    short events;
    short revents;
};

int poll(struct pollfd *fds, nfds_t nfds, int timeout);

#ifdef __cplusplus
}
#endif

#endif /* _MYOS_POLL_H_ */
