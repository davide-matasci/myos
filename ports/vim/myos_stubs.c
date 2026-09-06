/* Runtime stubs for Vim on myos (termios / select / misc). */
#include <errno.h>
#include <fcntl.h>
#include <string.h>
#include <sys/select.h>
#include <sys/types.h>
#include <unistd.h>
#include "termios.h"

int tcgetattr(int fd, struct termios *t) {
    (void)fd;
    if (t)
        memset(t, 0, sizeof(*t));
    errno = ENOTTY;
    return -1;
}

int tcsetattr(int fd, int optional_actions, const struct termios *t) {
    (void)fd;
    (void)optional_actions;
    (void)t;
    /* Pretend success so Vim believes cooked/raw switches worked; kernel
     * stdin stays cooked (same spirit as oksh). */
    return 0;
}

int tcsendbreak(int fd, int duration) {
    (void)fd;
    (void)duration;
    return 0;
}
int tcdrain(int fd) {
    (void)fd;
    return 0;
}
int tcflush(int fd, int queue_selector) {
    (void)fd;
    (void)queue_selector;
    return 0;
}
int tcflow(int fd, int action) {
    (void)fd;
    (void)action;
    return 0;
}
pid_t tcgetpgrp(int fd) {
    (void)fd;
    errno = ENOTTY;
    return (pid_t)-1;
}
int tcsetpgrp(int fd, pid_t pgrp) {
    (void)fd;
    (void)pgrp;
    errno = ENOTTY;
    return -1;
}
speed_t cfgetispeed(const struct termios *t) {
    (void)t;
    return 0;
}
speed_t cfgetospeed(const struct termios *t) {
    (void)t;
    return 0;
}
int cfsetispeed(struct termios *t, speed_t speed) {
    (void)t;
    (void)speed;
    return 0;
}
int cfsetospeed(struct termios *t, speed_t speed) {
    (void)t;
    (void)speed;
    return 0;
}

/* Minimal select: zero-timeout → nothing ready; otherwise report stdin
 * readable so Vim proceeds to a blocking read on cooked stdin. */
int select(int nfds, fd_set *readfds, fd_set *writefds, fd_set *exceptfds,
           struct timeval *timeout) {
    int ready = 0;
    (void)nfds;
    if (timeout && timeout->tv_sec == 0 && timeout->tv_usec == 0) {
        if (readfds)
            FD_ZERO(readfds);
        if (writefds)
            FD_ZERO(writefds);
        if (exceptfds)
            FD_ZERO(exceptfds);
        return 0;
    }
    if (writefds)
        FD_ZERO(writefds);
    if (exceptfds)
        FD_ZERO(exceptfds);
    if (readfds) {
        int want_stdin = (nfds > 0 && FD_ISSET(0, readfds));
        FD_ZERO(readfds);
        if (want_stdin) {
            FD_SET(0, readfds);
            ready = 1;
        }
    }
    return ready;
}

/* libgloss has dup2 / F_DUPFD but not dup(2). */
int dup(int oldfd) {
    return fcntl(oldfd, F_DUPFD, 0);
}

/* term_set_winsize lives under HAVE_TGETENT in term.c; without ncurses we
 * stub it so mch_set_shellsize links (no-op on dumb serial/FB). */
void term_set_winsize(int height, int width) {
    (void)height;
    (void)width;
}
