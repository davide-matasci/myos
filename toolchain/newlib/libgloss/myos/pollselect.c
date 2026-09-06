/*
 * poll/select for myos: no kernel poll syscall.
 * /net data fds already return 0 when empty (non-blocking). Always report
 * readiness so curl busy-waits like the existing http helper.
 */
#include <errno.h>
#include <poll.h>
#include <string.h>
#include <sys/select.h>
#include <sys/time.h>
#include <unistd.h>

int poll(struct pollfd *fds, nfds_t nfds, int timeout) {
    nfds_t i;
    int ready = 0;
    int spins;
    int s;

    if (fds == NULL && nfds != 0) {
        errno = EFAULT;
        return -1;
    }

    if (timeout < 0) {
        spins = 200000;
    } else if (timeout == 0) {
        spins = 1;
    } else {
        spins = timeout * 200;
        if (spins < 1) {
            spins = 1;
        }
        if (spins > 400000) {
            spins = 400000;
        }
    }

    for (s = 0; s < spins; s++) {
        ready = 0;
        for (i = 0; i < nfds; i++) {
            short rev = 0;
            if (fds[i].fd < 0) {
                fds[i].revents = 0;
                continue;
            }
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
        }
        if (ready > 0) {
            return ready;
        }
    }
    for (i = 0; i < nfds; i++) {
        fds[i].revents = 0;
    }
    return 0;
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
