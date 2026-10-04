/*
 * poll/select for myos: one SYS_POLL call does the waiting. The kernel knows
 * the readiness of every fd (pipes, ptys, the console tty, sockets through
 * netfs's poll hook; regular files are always ready) and sleeps until the
 * first change, so nothing here spins or sleeps in steps.
 *
 * Tracked sockets (socket.c) add what only the socket library knows: a
 * connected UDP socket is always writable, a listener
 * must arm netd's accept before waiting, and a finished connect changes the
 * socket's state. myos_socket_poll_prepare() runs before the kernel call,
 * myos_socket_poll_done() on its result.
 */
#include <errno.h>
#include <poll.h>
#include <string.h>
#include <sys/select.h>
#include <sys/time.h>
#include <unistd.h>

#include "myos_syscalls.h"

/* Most fds one poll() takes (the kernel's limit too). */
#define MYOS_POLL_MAX 256

static long elapsed_ms(const struct timeval *start) {
    struct timeval now;
    if (gettimeofday(&now, NULL) != 0) {
        return 0;
    }
    return (now.tv_sec - start->tv_sec) * 1000L
        + (now.tv_usec - start->tv_usec) / 1000L;
}

int poll(struct pollfd *fds, nfds_t nfds, int timeout) {
    struct pollfd k[MYOS_POLL_MAX];
    short now[MYOS_POLL_MAX];
    struct timeval start;
    nfds_t i;

    if (fds == NULL && nfds != 0) {
        errno = EFAULT;
        return -1;
    }
    if (nfds > MYOS_POLL_MAX) {
        errno = EINVAL;
        return -1;
    }
    if (timeout > 0 && gettimeofday(&start, NULL) != 0) {
        timeout = 0;
    }
    for (;;) {
        int immediate = 0;
        int left = timeout;
        int ready = 0;
        for (i = 0; i < nfds; i++) {
            short kev = fds[i].events;
            now[i] = 0;
            if (fds[i].fd >= 0
                && myos_socket_poll_prepare(fds[i].fd, fds[i].events, &now[i], &kev) == 0
                && now[i] != 0) {
                immediate = 1;
            }
            k[i].fd = fds[i].fd;
            k[i].events = kev;
            k[i].revents = 0;
        }
        if (timeout > 0) {
            left = timeout - (int)elapsed_ms(&start);
            if (left < 0) {
                left = 0;
            }
        }
        if (__myos_kpoll(k, nfds, immediate ? 0 : left) < 0) {
            return -1;
        }
        for (i = 0; i < nfds; i++) {
            short rev = k[i].revents;
            if (fds[i].fd >= 0) {
                myos_socket_poll_done(fds[i].fd, fds[i].events, &rev);
            }
            fds[i].revents = rev | now[i];
            if (fds[i].revents != 0) {
                ready++;
            }
        }
        if (ready > 0 || timeout == 0 || (timeout > 0 && elapsed_ms(&start) >= timeout)) {
            return ready;
        }
        /* The kernel saw something the socket library then discarded (a
         * listener's accept it already took): look again. */
    }
}

int select(int nfds, fd_set *readfds, fd_set *writefds,
    fd_set *exceptfds, struct timeval *timeout) {
    struct pollfd pfds[FD_SETSIZE];
    int n = 0;
    int i;
    int ms;
    int pr;

    if (nfds < 0 || nfds > FD_SETSIZE) {
        errno = EINVAL;
        return -1;
    }

    for (i = 0; i < nfds; i++) {
        short ev = 0;
        if (readfds && FD_ISSET(i, readfds)) {
            ev |= POLLIN;
        }
        if (writefds && FD_ISSET(i, writefds)) {
            ev |= POLLOUT;
        }
        if (exceptfds && FD_ISSET(i, exceptfds)) {
            ev |= POLLERR;
        }
        if (ev == 0) {
            continue;
        }
        pfds[n].fd = i;
        pfds[n].events = ev;
        pfds[n].revents = 0;
        n++;
    }

    
    if (timeout == NULL) {
        ms = -1;
    } else {
        ms = (int)(timeout->tv_sec * 1000 + timeout->tv_usec / 1000);
        if (ms < 0) {
            ms = 0;
        }
    }

    /* Pure sleep: select(0, NULL, NULL, NULL, &tv) (curl's tool_sleep) is a
     * poll of no fds, which a caught signal ends with EINTR. */
    if (n == 0) {
        return __myos_kpoll(NULL, 0, ms) < 0 ? -1 : 0;
    }

    pr = poll(pfds, (nfds_t)n, ms);
    if (pr < 0) {
        return -1;
    }

    if (readfds) {
        FD_ZERO(readfds);
    }
    if (writefds) {
        FD_ZERO(writefds);
    }
    if (exceptfds) {
        FD_ZERO(exceptfds);
    }

    pr = 0;
    for (i = 0; i < n; i++) {
        int fd = pfds[i].fd;
        if (pfds[i].revents & (POLLIN | POLLHUP | POLLERR)) {
            if (readfds) {
                FD_SET(fd, readfds);
            }
            pr++;
        }
        if (pfds[i].revents & POLLOUT) {
            if (writefds) {
                FD_SET(fd, writefds);
            }
            pr++;
        }
        if (pfds[i].revents & (POLLERR | POLLNVAL)) {
            if (exceptfds) {
                FD_SET(fd, exceptfds);
            }
        }
    }
    return pr;
}
