/*
 * poll/select for myos: no kernel poll syscall.
 * Tracked /net sockets report real POLLIN via netfs st_size / hangup status.
 * Regular files stay always-ready (POSIX). TTY POLLIN is not: there is no
 * FIONREAD, and lying "ready" made lynx HTCheckForInterrupt block in LYgetch
 * at "Looking up … first" before libgloss DNS (same getaddrinfo path as curl).
 * Timeouts use gettimeofday so select(0,...,tv) can sleep without a spin budget.
 */
#include <errno.h>
#include <poll.h>
#include <string.h>
#include <sys/select.h>
#include <sys/time.h>
#include <unistd.h>
#include <sys/stat.h>

#include "myos_syscalls.h"

/* Defined in misc_stubs.c: no userspace handler trampolines, so a custom
 * SIGCHLD handler is dispatched here when the kernel reports the pending bit.
 * Servers (dropbear) rely on this to reap command children and send exit-status. */
extern void myos_sigchld_dispatch(void);
extern int myos_sigchld_armed(void);
extern volatile long myos_sigchld_wfd;

static long elapsed_ms(const struct timeval *start) {
    struct timeval now;
    if (gettimeofday(&now, NULL) != 0) {
        return 0;
    }
    return (now.tv_sec - start->tv_sec) * 1000L
        + (now.tv_usec - start->tv_usec) / 1000L;
}

static int scan_once(struct pollfd *fds, nfds_t nfds) {
    nfds_t i;
    int ready = 0;
    for (i = 0; i < nfds; i++) {
        short rev = 0;
        int sock;
        if (fds[i].fd < 0) {
            fds[i].revents = 0;
            continue;
        }
        sock = myos_socket_poll(fds[i].fd, fds[i].events, &rev);
        if (sock == 1) {
            fds[i].revents = rev;
            ready++;
            continue;
        }
        if (sock == -2) {
            return -1;
        }
        /* Not a tracked socket (or not ready).
         * Regular files: POSIX always-ready.
         * TTY POLLIN: do NOT lie. Lynx HTCheckForInterrupt does
         * select(stdin, timeout=0) after painting "Looking up … first";
         * always-ready made it call blocking LYgetch() and never reach
         * gethostbyname / getaddrinfo. No FIONREAD yet, so report not-ready
         * (false negative: 'z' during a transfer is missed) rather than hang.
         */
        if (sock < 0) {
            /* Not a tracked socket. Only regular files are POSIX
             * always-ready; blocking fds we cannot verify (pipes, ttys)
             * must report not-ready, or select-driven servers wake on an
             * empty signal pipe and hang in a blocking read() forever
             * (dropbear's session_loop signal-pipe drain). */
            rev = 0;
            /* Only fds with a known path (regular files we opened) are
             * POSIX always-ready. The _fstat fallback reports every
             * pathless fd (pipes!) as S_IFREG, so fstat alone would
             * still wake select() on an empty signal pipe and the
             * caller's blocking read() hangs forever (dropbear). */
            const char *fpath = myos_fd_path_get(fds[i].fd);
            if (fpath != NULL) {
                rev = fds[i].events ? fds[i].events : (POLLIN | POLLOUT);
            } else {
                /* Pathless blocking fd: pipes. Ask the kernel for real
                 * readiness so select-driven servers relay a forked
                 * command's output (dropbear session stdout) and still do
                 * not spuriously wake on an empty signal pipe. */
                long bits = myos_syscall1(MYOS_SYS_POLLFD, fds[i].fd);
                if (bits != (long)MYOS_SYSERR && bits > 0) {
                    if ((bits & 1) && (fds[i].events & POLLIN)) {
                        rev |= POLLIN;
                    }
                    if ((bits & 2) && (fds[i].events & POLLOUT)) {
                        rev |= POLLOUT;
                    }
                    if (bits & 4) {
                        rev |= POLLHUP;
                    }
                }
            }
            /* Deterministic SIGCHLD wake: dropbear sets channel_signal_pending
             * whenever FD_ISSET(signal_pipe[0]) is true (it doesn't need data).
             * A blocked select() isn't interrupted on myos, so while the caller
             * has an exited child, report its pipe read fds as POLLIN to make
             * select() return >0 and let dropbear notice the signal pipe. */
            if (rev == 0 && myos_sigchld_armed()
                && (fds[i].events & POLLIN)
                && myos_syscall0(MYOS_SYS_SIGCHLD_PENDING) == 1) {
                /* Force exactly the SIGCHLD handler's self-pipe read end
                 * readable, so dropbear's select() returns >0 with
                 * signal_pipe[0] set and it reaps the child/sends exit-status.
                 * The handler ran earlier this iteration (poll() dispatches
                 * before scanning), so its write fd is captured. Only the
                 * peer read end is forced — never every pathless fd (that
                 * used to wake the drain on an empty pipe). */
                long wfd = myos_sigchld_wfd;
                long peer = (wfd >= 0)
                    ? myos_syscall1(MYOS_SYS_PIPE_PEER, wfd) : (long)MYOS_SYSERR;
                if (wfd >= 0 && peer != (long)MYOS_SYSERR
                    && peer == (long)fds[i].fd) {
                    rev = POLLIN;
                }
            }
            fds[i].revents = rev;
            if (rev) {
                ready++;
            }
        } else {
            fds[i].revents = 0;
        }
    }
    return ready;
}

int poll(struct pollfd *fds, nfds_t nfds, int timeout) {
    struct timeval start;
    int ready;

    
    if (fds == NULL && nfds != 0) {
        errno = EFAULT;
        return -1;
    }

    /* Also dispatch on entry: the timeout==0 fast path below returns without
     * reaching the loop, and a select-driven server can keep calling with a
     * zero timeout (select_timeout() due), which would otherwise starve
     * SIGCHLD reaping (dropbear never sends an SSH exit-status). */
    myos_sigchld_dispatch();

    if (timeout == 0) {
        ready = scan_once(fds, nfds);
        return ready < 0 ? -1 : ready;
    }

    /* Cap a long/blocking wait when a SIGCHLD handler is armed: a child exit
     * does not interrupt a blocked select() on myos, so we poll for it. */
    if (timeout < 0 || timeout > 100) {
        if (myos_sigchld_armed()) {
            timeout = 100;
        }
    }

    if (gettimeofday(&start, NULL) != 0) {
        /* Clock missing: single scan (best effort). */
        ready = scan_once(fds, nfds);
        return ready < 0 ? -1 : ready;
    }

    for (;;) {
        myos_sigchld_dispatch();
        ready = scan_once(fds, nfds);
        if (ready < 0) {
            return -1;
        }
        if (ready > 0) {
            return ready;
        }
        if (timeout > 0 && elapsed_ms(&start) >= timeout) {
            nfds_t i;
            for (i = 0; i < nfds; i++) {
                fds[i].revents = 0;
            }
            return 0;
        }
        /* Infinite wait: scan_once may be pure userspace on idle sockets.
         * Touch the clock so each spin enters the kernel and netd can run
         * (riscv64 dropbear select starved netd → accept never completed,
         * then Child+banner write EIO when the ring/socket was already dead). */
        {
            struct timeval now;
            (void)gettimeofday(&now, NULL);
        }
    }
}

int select(int nfds, fd_set *readfds, fd_set *writefds,
    fd_set *exceptfds, struct timeval *timeout) {
    struct pollfd pfds[FD_SETSIZE];
    int n = 0;
    int i;
    int ms;
    int pr;

    if (nfds < 0 || nfds > FD_SETSIZE) {
        errno = EINVAL;
        return -1;
    }

    for (i = 0; i < nfds; i++) {
        short ev = 0;
        if (readfds && FD_ISSET(i, readfds)) {
            ev |= POLLIN;
        }
        if (writefds && FD_ISSET(i, writefds)) {
            ev |= POLLOUT;
        }
        if (exceptfds && FD_ISSET(i, exceptfds)) {
            ev |= POLLERR;
        }
        if (ev == 0) {
            continue;
        }
        pfds[n].fd = i;
        pfds[n].events = ev;
        pfds[n].revents = 0;
        n++;
    }

    
    if (timeout == NULL) {
        ms = -1;
    } else {
        ms = (int)(timeout->tv_sec * 1000 + timeout->tv_usec / 1000);
        if (ms < 0) {
            ms = 0;
        }
    }

    /* Pure sleep: select(0, NULL, NULL, NULL, &tv) used by curl tool_sleep. */
    if (n == 0) {
        struct timeval start;
        if (ms == 0) {
            myos_sigchld_dispatch();
            return 0;
        }
        if (gettimeofday(&start, NULL) != 0) {
            return 0;
        }
        /* Sleep, but dispatch SIGCHLD and wake the caller if a child exited so
         * its next select() includes the signal pipe (myos has no trampolines). */
        while (ms < 0 || elapsed_ms(&start) < ms) {
            myos_sigchld_dispatch();
            if (myos_sigchld_armed()
                && myos_syscall0(MYOS_SYS_SIGCHLD_PENDING) == 1) {
                return 0;
            }
            if (ms >= 0 && elapsed_ms(&start) >= ms) {
                break;
            }
        }
        return 0;
    }

    pr = poll(pfds, (nfds_t)n, ms);
    if (pr < 0) {
        return -1;
    }

    if (readfds) {
        FD_ZERO(readfds);
    }
    if (writefds) {
        FD_ZERO(writefds);
    }
    if (exceptfds) {
        FD_ZERO(exceptfds);
    }

    pr = 0;
    for (i = 0; i < n; i++) {
        int fd = pfds[i].fd;
        if (pfds[i].revents & (POLLIN | POLLHUP | POLLERR)) {
            if (readfds) {
                FD_SET(fd, readfds);
            }
            pr++;
        }
        if (pfds[i].revents & POLLOUT) {
            if (writefds) {
                FD_SET(fd, writefds);
            }
            pr++;
        }
        if (pfds[i].revents & (POLLERR | POLLNVAL)) {
            if (exceptfds) {
                FD_SET(fd, exceptfds);
            }
        }
    }
    return pr;
}
