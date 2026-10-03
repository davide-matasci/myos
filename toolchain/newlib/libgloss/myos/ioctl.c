/* myos libgloss: the tty ioctls ported programs call (vim, getty, login,
 * stty, openpty), served from the terminal's files (ttyctl.c, docs/tty.md).
 * There is no ioctl syscall behind this: a request that is not one of
 * these is ENOTTY. */
#include <errno.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <termios.h>
#include <unistd.h>

#include "myos_syscalls.h"

int ioctl(int fd, unsigned long request, ...) {
    va_list ap;
    void *arg;
    char dir[MYOS_TTY_PATH];
    char line[64];
    int master;

    if (fd < 0) {
        errno = EBADF;
        return -1;
    }

    va_start(ap, request);
    arg = va_arg(ap, void *);
    va_end(ap);

    switch (request) {
    case TCGETS:
        return myos_tty_get(fd, (struct termios *)arg, NULL);
    case TCSETS:
        return myos_tty_set(fd, (const struct termios *)arg);
    case TCFLSH:
        return tcflush(fd, (int)(long)arg);
    case TIOCGWINSZ:
        if (arg == NULL) {
            errno = EFAULT;
            return -1;
        }
        return myos_tty_get(fd, NULL, (struct winsize *)arg);
    case TIOCSWINSZ: {
        const struct winsize *w = (const struct winsize *)arg;
        if (w == NULL) {
            errno = EFAULT;
            return -1;
        }
        snprintf(line, sizeof line, "winsize %u %u\n", w->ws_row, w->ws_col);
        return myos_tty_write(fd, line);
    }
    case TIOCSCTTY:
        return myos_tty_write(fd, "ctty\n");
    case TIOCGPTN:
        /* The pair's index is the master's directory name, /dev/pts/N. */
        if (myos_tty_dir(fd, dir, sizeof dir, &master) != 0) {
            return -1;
        }
        if (!master || arg == NULL) {
            errno = ENOTTY;
            return -1;
        }
        *(unsigned int *)arg = (unsigned int)strtoul(strrchr(dir, '/') + 1, NULL, 10);
        return 0;
    case TIOCSPTLCK:
        /* No lock to lift: the slave opens as soon as the pair exists. */
        return myos_tty_dir(fd, NULL, 0, NULL);
    default:
        errno = ENOTTY;
        return -1;
    }
}
