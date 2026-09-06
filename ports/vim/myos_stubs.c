/* Runtime stubs for Vim on myos (select / misc).
 * termios lives in libgloss (<termios.h> + termios.c). */
#include <fcntl.h>
#include <sys/select.h>
#include <sys/types.h>
#include <unistd.h>

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

