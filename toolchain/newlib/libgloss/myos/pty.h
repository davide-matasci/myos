/* myos libgloss: pty allocation (kernel-backed /dev/ptmx + /dev/pts/N). */
#ifndef _MYOS_PTY_H_
#define _MYOS_PTY_H_

#include <sys/ioctl.h>
#include <termios.h>
#include <unistd.h>

/* Allocates a pty pair: opens /dev/ptmx (master) and the matching
 * /dev/pts/N (slave). `name` (if non-NULL) receives the slave path.
 * `termp`/`winp` (if non-NULL) are applied to the slave. Returns 0 and
 * fills both fds, or -1 with errno. */
int openpty(int *amaster, int *aslave, char *name,
            const struct termios *termp, const struct winsize *winp);

/* fork(2) a child with the pty slave as its controlling terminal and
 * std{in,out,err}; the child returns 0 from forkpty, the parent returns
 * the child pid with *amaster set. */
int forkpty(int *amaster, char *name,
            const struct termios *termp, const struct winsize *winp);

#endif /* _MYOS_PTY_H_ */
