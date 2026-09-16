/* myos libgloss: pty allocation (openpty/forkpty) via /dev/ptmx + /dev/pts/N.
 *
 * Kernel model (see kernel/src/pty.rs): open("/dev/ptmx") allocates a pair
 * and returns the master fd; TIOCGPTN yields the slave index N; open of
 * "/dev/pts/N" takes a slave fd and (first open) claims the session.
 * Master reads block and return EIO once the last slave fd closes; slave
 * reads do the same once the master closes. TIOCSCTTY on the slave makes the
 * caller the session leader (SIGINT/SIGHUP scoping).
 */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/types.h>
#include <termios.h>
#include <unistd.h>

#include "myos_syscalls.h"

/* Linux pty ioctls (kernel-backed). TIOCGPTN lives in <sys/ioctl.h> on
 * glibc; myos ships it here to keep one definition per header tree. */
#ifndef TIOCGPTN
#define TIOCGPTN 0x80045430
#endif
#ifndef TIOCSPTLCK
#define TIOCSPTLCK 0x40045431
#endif

int openpty(int *amaster, int *aslave, char *name,
            const struct termios *termp, const struct winsize *winp) {
    int m = -1, s = -1;
    unsigned int n = 0;
    char slave_path[32];

    /* glibc openpty: aslave may be NULL (the internal slave fd is closed
     * before return); only amaster is required. */
    if (amaster == NULL) {
        errno = EINVAL;
        return -1;
    }

    m = open("/dev/ptmx", O_RDWR | O_NOCTTY);
    if (m < 0) {
        return -1;
    }

    if (ioctl(m, TIOCGPTN, &n) != 0) {
        goto fail;
    }

    /* grantpt/unlockpt are implicit: the kernel enforces no lock and the
     * myos ABI has no privilege split (phase-1 single-user). */

    if (snprintf(slave_path, sizeof(slave_path), "/dev/pts/%u", n) >= (int)sizeof(slave_path)) {
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
        /* Best effort: full slave path for callers that print it. */
        snprintf(name, sizeof("/dev/pts/00"), "%s", slave_path);
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
