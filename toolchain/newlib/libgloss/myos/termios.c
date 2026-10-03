/* myos libgloss: termios over the terminal's ctl file (ttyctl.c). */
#include <errno.h>
#include <string.h>
#include <sys/ioctl.h>
#include <termios.h>
#include <unistd.h>

#include "myos_syscalls.h"

int tcgetattr(int fd, struct termios *t) {
    if (!t) {
        errno = EINVAL;
        return -1;
    }
    return myos_tty_get(fd, t, NULL);
}

int tcsetattr(int fd, int optional_actions, const struct termios *t) {
    (void)optional_actions; /* TCSANOW / DRAIN / FLUSH: kernel applies immediately */
    if (!t) {
        errno = EINVAL;
        return -1;
    }
    return myos_tty_set(fd, t);
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
    const char *line;
    switch (queue_selector) {
    case TCIFLUSH:
        line = "flush in\n";
        break;
    case TCOFLUSH:
        line = "flush out\n";
        break;
    case TCIOFLUSH:
        line = "flush both\n";
        break;
    default:
        errno = EINVAL;
        return -1;
    }
    return myos_tty_write(fd, line);
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
    return t ? t->c_ispeed : 0;
}

speed_t cfgetospeed(const struct termios *t) {
    return t ? t->c_ospeed : 0;
}

int cfsetispeed(struct termios *t, speed_t speed) {
    if (!t)
        return -1;
    t->c_ispeed = speed;
    return 0;
}

int cfsetospeed(struct termios *t, speed_t speed) {
    if (!t)
        return -1;
    t->c_ospeed = speed;
    return 0;
}
