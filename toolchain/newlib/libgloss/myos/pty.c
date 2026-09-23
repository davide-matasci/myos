/* myos libgloss: pty allocation (openpty/forkpty) via /dev/ptmx + /dev/pts/N,
 * plus the POSIX pty allocation API (posix_openpt/grantpt/unlockpt/ptsname).
 *
 * Kernel model (see kernel/src/pty.rs): open("/dev/ptmx") allocates a pair
 * and returns the master fd; TIOCGPTN yields the slave index N; open of
 * "/dev/pts/N" takes a slave fd and, without O_NOCTTY, binds the caller's
 * session to the pair (initial foreground group). TIOCSCTTY on the slave
 * claims or steals it (SIGINT/SIGHUP scoping, TIOCGPGRP/TIOCSPGRP).
 * Master reads block and return EIO once the last slave fd closes; slave
 * reads do the same once the master closes.
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

/* POSIX pty allocation API. posix_openpt is a plain open(2) of /dev/ptmx
 * (flags pass through, O_NOCTTY included). grantpt/unlockpt are no-ops that
 * route through the kernel's TIOCSPTLCK (the myos ABI has no privilege
 * split, so there is no ownership check to perform). ptsname/ptsname_r map
 * TIOCGPTN onto the /dev/pts/N slave path. */
int posix_openpt(int flags) { return open("/dev/ptmx", flags); }

int grantpt(int fd) {
    (void)fd;
    /* Kernel-enforced: no lock and no ownership handshake to perform. */
    return 0;
}

int unlockpt(int fd) {
    /* Clear the kernel's lock flag (always accepted) to mirror Linux. */
    if (ioctl(fd, TIOCSPTLCK, 0) < 0) {
        return -1;
    }
    return 0;
}

int ptsname_r(int fd, char *buf, size_t size) {
    unsigned int n = 0;
    char path[32];

    if (buf == NULL || size == 0) {
        errno = EINVAL;
        return errno;
    }
    if (ioctl(fd, TIOCGPTN, &n) != 0) {
        return errno;
    }
    if (snprintf(path, sizeof(path), "/dev/pts/%u", n) >= (int)sizeof(path)) {
        errno = ENAMETOOLONG;
        return errno;
    }
    if (strlen(path) + 1 > size) {
        errno = ERANGE;
        return errno;
    }
    strcpy(buf, path);
    return 0;
}

char *ptsname(int fd) {
    static char name[32];

    if (ptsname_r(fd, name, sizeof(name)) != 0) {
        return NULL;
    }
    return name;
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
