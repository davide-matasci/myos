/* termios over the terminal's ctl file (ttyctl.c). */
#include <errno.h>
#include <string.h>
#include <sys/ioctl.h>
#include <termios.h>
#include <unistd.h>

#include "myos_syscalls.h"
#include "libc.h"

int __myos_tcgetattr(int fd, struct termios *t) {
    if (!t) {
        errno = EINVAL;
        return -1;
    }
    return myos_tty_get(fd, t, NULL);
}

int __myos_tcsetattr(int fd, int optional_actions, const struct termios *t) {
    (void)optional_actions; /* TCSANOW / DRAIN / FLUSH: kernel applies immediately */
    if (!t) {
        errno = EINVAL;
        return -1;
    }
    return myos_tty_set(fd, t);
}

int __myos_tcsendbreak(int fd, int duration) {
    (void)fd;
    (void)duration;
    return 0;
}

int __myos_tcdrain(int fd) {
    (void)fd;
    return 0;
}

int __myos_tcflush(int fd, int queue_selector) {
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

int __myos_tcflow(int fd, int action) {
    (void)fd;
    (void)action;
    return 0;
}

pid_t __myos_tcgetpgrp(int fd) {
    (void)fd;
    errno = ENOTTY;
    return (pid_t)-1;
}

int __myos_tcsetpgrp(int fd, pid_t pgrp) {
    (void)fd;
    (void)pgrp;
    errno = ENOTTY;
    return -1;
}

speed_t __myos_cfgetispeed(const struct termios *t) {
    return t ? t->c_ispeed : 0;
}

speed_t __myos_cfgetospeed(const struct termios *t) {
    return t ? t->c_ospeed : 0;
}

int __myos_cfsetispeed(struct termios *t, speed_t speed) {
    if (!t)
        return -1;
    t->c_ispeed = speed;
    return 0;
}

int __myos_cfsetospeed(struct termios *t, speed_t speed) {
    if (!t)
        return -1;
    t->c_ospeed = speed;
    return 0;
}

MYOS_ALIAS(tcgetattr)
MYOS_ALIAS(tcsetattr)
MYOS_ALIAS(tcsendbreak)
MYOS_ALIAS(tcdrain)
MYOS_ALIAS(tcflush)
MYOS_ALIAS(tcflow)
MYOS_ALIAS(tcgetpgrp)
MYOS_ALIAS(tcsetpgrp)
MYOS_ALIAS(cfgetispeed)
MYOS_ALIAS(cfgetospeed)
MYOS_ALIAS(cfsetispeed)
MYOS_ALIAS(cfsetospeed)
