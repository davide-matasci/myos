/*
 * poll/select for myos: no kernel poll syscall.
 * Tracked /net sockets report real POLLIN via netfs st_size / hangup status.
 * Other fds keep the legacy always-ready busy-wait (matches http helper).
 * Timeouts use gettimeofday so select(0,...,tv) can sleep without a spin budget.
 */
#include <errno.h>
#include <poll.h>
#include <string.h>
#include <sys/select.h>
#include <sys/time.h>
#include <unistd.h>

#include "myos_syscalls.h"

static long elapsed_ms(const struct timeval *start) {
    struct timeval now;
    if (gettimeofday(&now, NULL) != 0) {
        return 0;
    }
    return (now.tv_sec - start->tv_sec) * 1000L
        + (now.tv_usec - start->tv_usec) / 1000L;
}

static int scan_once(struct pollfd *fds, nfds_t nfds) {
    nfds_t i;
    int ready = 0;
    for (i = 0; i < nfds; i++) {
        short rev = 0;
        int sock;
        if (fds[i].fd < 0) {
            fds[i].revents = 0;
            continue;
        }
        sock = myos_socket_poll(fds[i].fd, fds[i].events, &rev);
        if (sock == 1) {
            fds[i].revents = rev;
            ready++;
            continue;
        }
        if (sock == -2) {
            return -1;
        }
        /* Not a tracked socket (or not ready): legacy always-ready for non-sockets. */
        if (sock < 0) {
            rev = 0;
            if (fds[i].events & (POLLIN | POLLPRI | POLLRDNORM)) {
                rev |= POLLIN;
            }
            if (fds[i].events & (POLLOUT | POLLWRNORM)) {
                rev |= POLLOUT;
            }
            if (rev == 0 && fds[i].events == 0) {
                rev = POLLIN | POLLOUT;
            }
            fds[i].revents = rev;
            if (rev) {
                ready++;
            }
        } else {
            fds[i].revents = 0;
        }
    }
    return ready;
}

int poll(struct pollfd *fds, nfds_t nfds, int timeout) {
    struct timeval start;
    int ready;

    if (fds == NULL && nfds != 0) {
        errno = EFAULT;
        return -1;
    }

    if (timeout == 0) {
        ready = scan_once(fds, nfds);
        return ready < 0 ? -1 : ready;
    }

    if (gettimeofday(&start, NULL) != 0) {
        /* Clock missing: single scan (best effort). */
        ready = scan_once(fds, nfds);
        return ready < 0 ? -1 : ready;
    }

    for (;;) {
        ready = scan_once(fds, nfds);
        if (ready < 0) {
            return -1;
        }
        if (ready > 0) {
            return ready;
        }
        if (timeout > 0 && elapsed_ms(&start) >= timeout) {
            nfds_t i;
            for (i = 0; i < nfds; i++) {
                fds[i].revents = 0;
            }
            return 0;
        }
        /* timeout < 0: block forever until ready (preemption lets netd run). */
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

    /* Pure sleep: select(0, NULL, NULL, NULL, &tv) used by curl tool_sleep. */
    if (n == 0) {
        struct timeval start;
        if (ms == 0) {
            return 0;
        }
        if (ms < 0) {
            /* Sleep forever — park in a gettimeofday loop (preemptible). */
            for (;;) {
            }
        }
        if (gettimeofday(&start, NULL) != 0) {
            return 0;
        }
        while (elapsed_ms(&start) < ms) {
        }
        return 0;
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
