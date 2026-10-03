/* myos libgloss: pty allocation (openpty/forkpty) over /dev/pts (docs/tty.md).
 *
 * Opening /dev/pts/clone allocates a pair and returns the master fd; the
 * master's /proc/self/fd link names the pair's directory /dev/pts/N, whose
 * `data` is the slave. The slave's first open does not claim the session:
 * forkpty's child does, with TIOCSCTTY (a `ctty` line on the pair's ctl).
 * Master reads block and return EIO once the last slave fd closes; slave
 * reads do the same once the master closes. */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <termios.h>
#include <unistd.h>

#include "myos_syscalls.h"
#include "pty.h"

int openpty(int *amaster, int *aslave, char *name,
            const struct termios *termp, const struct winsize *winp) {
    int m = -1, s = -1;
    char dir[MYOS_TTY_PATH];
    char slave_path[MYOS_TTY_PATH];

    /* glibc openpty: aslave may be NULL (the internal slave fd is closed
     * before return); only amaster is required. */
    if (amaster == NULL) {
        errno = EINVAL;
        return -1;
    }

    m = open("/dev/pts/clone", O_RDWR | O_NOCTTY);
    if (m < 0) {
        return -1;
    }
    if (myos_tty_dir(m, dir, sizeof dir, NULL) != 0) {
        goto fail;
    }

    /* grantpt/unlockpt are implicit: there is no lock and the myos ABI has
     * no privilege split (phase-1 single-user). */

    if (snprintf(slave_path, sizeof slave_path, "%s/data", dir) >= (int)sizeof slave_path) {
        errno = ENAMETOOLONG;
        goto fail;
    }
    s = open(slave_path, O_RDWR | O_NOCTTY);
    if (s < 0) {
        goto fail;
    }

    if (termp != NULL) {
        (void)tcsetattr(s, TCSANOW, termp);
    }
    if (winp != NULL) {
        (void)ioctl(s, TIOCSWINSZ, winp);
    }

    if (name != NULL) {
        /* The slave's path, for callers that print it (at most 4 pairs, so
         * it is /dev/pts/N/data). */
        strcpy(name, slave_path);
    }

    *amaster = m;
    if (aslave != NULL) {
        *aslave = s;
    } else {
        /* glibc closes the internally-opened slave when the caller does not
         * want the fd. The pair stays alive via the master ref. */
        close(s);
    }
    return 0;

fail: {
        int saved = errno;
        if (s >= 0) close(s);
        if (m >= 0) close(m);
        errno = saved;
        return -1;
    }
}

int forkpty(int *amaster, char *name,
            const struct termios *termp, const struct winsize *winp) {
    int m = -1, s = -1;
    pid_t pid;

    if (openpty(&m, &s, name, termp, winp) != 0) {
        return -1;
    }

    pid = fork();
    if (pid < 0) {
        int saved = errno;
        close(m);
        close(s);
        errno = saved;
        return -1;
    }
    if (pid == 0) {
        /* Child: new session, claim the pty as ctty, wire std fds. */
        close(m);
        setsid();
        (void)ioctl(s, TIOCSCTTY, 0);
        if (s != 0) {
            dup2(s, 0);
        }
        if (s != 1) {
            dup2(s, 1);
        }
        if (s != 2) {
            dup2(s, 2);
        }
        if (s > 2) {
            close(s);
        }
        return 0;
    }

    /* Parent: drop the slave fd, keep the master. */
    close(s);
    *amaster = m;
    return pid;
}
