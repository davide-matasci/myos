#ifndef _SYS_EPOLL_H
#define _SYS_EPOLL_H

/* epoll(7) over /dev/wait (docs/events.md, evfd.c): an fd on a kernel wait
 * set, whose members are other fds' open objects. epoll_wait gives the
 * members ready now, with the data they were added with. EPOLLET reports
 * a readiness once, and again only once something happened since (a few
 * more than Linux's every-new-event on a busy system); EPOLLONESHOT once,
 * until EPOLL_CTL_MOD. EPOLLERR and EPOLLHUP are always reported;
 * EPOLLPRI, EPOLLRDHUP, EPOLLEXCLUSIVE and EPOLLWAKEUP are accepted and
 * ignored. */
#include <fcntl.h>
#include <signal.h>
#include <stdint.h>

#define EPOLL_CLOEXEC O_CLOEXEC

#define EPOLL_CTL_ADD 1
#define EPOLL_CTL_DEL 2
#define EPOLL_CTL_MOD 3

#define EPOLLIN 0x001
#define EPOLLPRI 0x002
#define EPOLLOUT 0x004
#define EPOLLERR 0x008
#define EPOLLHUP 0x010
#define EPOLLRDNORM 0x040
#define EPOLLRDBAND 0x080
#define EPOLLWRNORM 0x100
#define EPOLLWRBAND 0x200
#define EPOLLMSG 0x400
#define EPOLLRDHUP 0x2000
#define EPOLLEXCLUSIVE (1U << 28)
#define EPOLLWAKEUP (1U << 29)
#define EPOLLONESHOT (1U << 30)
#define EPOLLET (1U << 31)

typedef union epoll_data {
    void *ptr;
    int fd;
    uint32_t u32;
    uint64_t u64;
} epoll_data_t;

struct epoll_event {
    uint32_t events;
    epoll_data_t data;
};

#ifdef __cplusplus
extern "C" {
#endif

int epoll_create(int size);
int epoll_create1(int flags);
int epoll_ctl(int epfd, int op, int fd, struct epoll_event *event);
int epoll_wait(int epfd, struct epoll_event *events, int maxevents, int timeout);
int epoll_pwait(int epfd, struct epoll_event *events, int maxevents, int timeout, const sigset_t *sigmask);

#ifdef __cplusplus
}
#endif

#endif /* _SYS_EPOLL_H */
