/* myos libgloss: termios via TCGETS/TCSETS (kernel line discipline). */
#include <errno.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/types.h>
#include <termios.h>
#include <unistd.h>

int tcgetattr(int fd, struct termios *t) {
    if (!t) {
        errno = EINVAL;
        return -1;
    }
    if (ioctl(fd, TCGETS, t) < 0) {
        return -1;
    }
    return 0;
}

int tcsetattr(int fd, int optional_actions, const struct termios *t) {
    (void)optional_actions; /* TCSANOW / DRAIN / FLUSH: kernel applies immediately */
    if (!t) {
        errno = EINVAL;
        return -1;
    }
    if (ioctl(fd, TCSETS, (void *)t) < 0) {
        return -1;
    }
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
    if (ioctl(fd, TCFLSH, (void *)(long)queue_selector) < 0) {
        return -1;
    }
    return 0;
}

int tcflow(int fd, int action) {
    (void)fd;
    (void)action;
    return 0;
}

/* Foreground process group of the terminal behind fd (TIOCGPGRP). The
 * kernel stores it per pty pair and reports 0 for a session-less pair
 * (sortix/os-test pty/tcgetpgrp-uncontrolled requires >= 0). */
pid_t tcgetpgrp(int fd) {
    pid_t pgrp;
    if (ioctl(fd, TIOCGPGRP, &pgrp) < 0) {
        return (pid_t)-1;
    }
    return pgrp;
}

/* tcsetpgrp(3) maps onto TIOCSPGRP. The kernel stores the pgid verbatim:
 * the sortix/os-test pty suite (tcsetpgrp-wrong-pid/-wrong-session/-zombie/
 * -limbo/-wrong-orphan) requires permissive semantics — a nonexistent pid,
 * a foreign session's group, an awaited zombie leader, and an orphaned
 * caller must all succeed without SIGTTOU. pgid == 0 means the caller's
 * own process group (resolved kernel-side). */
int tcsetpgrp(int fd, pid_t pgrp) {
    if (ioctl(fd, TIOCSPGRP, &pgrp) < 0) {
        return -1;
    }
    return 0;
}

/* Session id of the terminal fd's pair (TIOCGSID): the sortix/os-test pty
 * suite requires tcgetsid(3) to succeed on session-less ptys (0) and to
 * return the pair's session from outside the session, so this is a plain
 * pair query, not a controlling-terminal check on the caller. */
pid_t tcgetsid(int fd) {
    pid_t sid;
    if (ioctl(fd, TIOCGSID, &sid) < 0) {
        return (pid_t)-1;
    }
    return sid;
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
