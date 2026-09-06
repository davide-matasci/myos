/* myos libgloss: termios stubs (kernel stdin is cooked). */
#include <errno.h>
#include <string.h>
#include <termios.h>
#include <unistd.h>

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
    /* Pretend success so callers believe cooked/raw switches worked. */
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
