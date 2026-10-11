/* sigfd-smoke: boot-CI guest test for the signal fds (/dev/sig) behind
 * signalfd(2) (kernel/src/evfd.rs, libgloss evfd.c, docs/events.md).
 *
 * A blocked signal raised at the process makes its fd readable (poll says
 * POLLIN, not before) and a read takes it, as a record with its number,
 * instead of its delivery; a signal outside the mask does not; an empty
 * nonblocking read is EAGAIN; a child's kill after a delay ends a blocked
 * read (not before); the mask changes with signalfd(fd, ...); the fd in a
 * wait set is reported; the status lists the mask. Prints [ OK ] sigfd.
 */
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/signalfd.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define DELAY_MS 300

static int fail(const char *what) {
    printf("[ FAIL ] sigfd %s (errno %d)\n", what, errno);
    return 1;
}

static long now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1000L + ts.tv_nsec / 1000000L;
}

static int poll_in(int fd, int timeout_ms) {
    struct pollfd p = {fd, POLLIN, 0};
    int n = poll(&p, 1, timeout_ms);
    return n < 0 ? -1 : n == 0 ? 0 : p.revents;
}

static int read_status(int fd, char *buf, size_t cap) {
    char link[32], path[64];
    ssize_t n;
    int sfd;
    snprintf(link, sizeof link, "/proc/self/fd/%d", fd);
    n = readlink(link, path, sizeof path - 8);
    if (n <= 5) {
        return -1;
    }
    path[n] = '\0';
    strcpy(path + n - 5, "/status");
    sfd = open(path, O_RDONLY);
    if (sfd < 0) {
        return -1;
    }
    n = read(sfd, buf, cap - 1);
    close(sfd);
    if (n < 0) {
        return -1;
    }
    buf[n] = '\0';
    return 0;
}

static volatile int usr2_handled;

static void on_usr2(int sig) {
    (void)sig;
    usr2_handled = 1;
}

int main(void) {
    struct signalfd_siginfo si;
    struct epoll_event ev;
    sigset_t set, two;
    char status[64];
    long t0;
    pid_t pid;
    int fd, ep, st = 0;

    sigemptyset(&set);
    sigaddset(&set, SIGUSR1);
    if (sigprocmask(SIG_BLOCK, &set, NULL) != 0) {
        return fail("sigprocmask");
    }
    fd = signalfd(-1, &set, SFD_NONBLOCK | SFD_CLOEXEC);
    if (fd < 0) {
        return fail("signalfd");
    }
    if (read_status(fd, status, sizeof status) != 0 || strcmp(status, "mask 30\n") != 0) {
        printf("status: [%s]\n", status);
        return fail("status lists the mask");
    }
    if (poll_in(fd, 0) != 0) {
        return fail("nothing pending: not readable");
    }
    if (read(fd, &si, sizeof si) != -1 || errno != EAGAIN) {
        return fail("an empty nonblocking read is EAGAIN");
    }
    if (read(fd, &si, 8) != -1) {
        return fail("a short read is refused");
    }
    if (kill(getpid(), SIGUSR1) != 0 || poll_in(fd, 1000) != POLLIN) {
        return fail("a pending blocked signal makes the fd readable");
    }
    if (read(fd, &si, sizeof si) != (ssize_t)sizeof si || si.ssi_signo != SIGUSR1) {
        return fail("a read takes the signal");
    }
    if (poll_in(fd, 0) != 0) {
        return fail("taken: not readable");
    }
    /* A signal outside the mask is delivered, not read. */
    signal(SIGUSR2, on_usr2);
    if (kill(getpid(), SIGUSR2) != 0 || !usr2_handled || poll_in(fd, 0) != 0) {
        return fail("a signal outside the mask is delivered");
    }
    /* A child's kill ends a blocked read, not before. */
    fcntl(fd, F_SETFL, 0);
    t0 = now_ms();
    pid = fork();
    if (pid == 0) {
        usleep(DELAY_MS * 1000);
        _exit(kill(getppid(), SIGUSR1) == 0 ? 0 : 1);
    }
    if (read(fd, &si, sizeof si) != (ssize_t)sizeof si || si.ssi_signo != SIGUSR1) {
        return fail("a blocked read gets the child's signal");
    }
    if (now_ms() - t0 < DELAY_MS / 2) {
        return fail("the read returned before the signal");
    }
    if (waitpid(pid, &st, 0) != pid || st != 0) {
        return fail("child");
    }
    /* The mask changes; a wait set sees the fd. */
    sigemptyset(&two);
    sigaddset(&two, SIGUSR1);
    sigaddset(&two, SIGUSR2);
    if (sigprocmask(SIG_BLOCK, &two, NULL) != 0 || signalfd(fd, &two, 0) != fd) {
        return fail("signalfd changes the mask");
    }
    if (read_status(fd, status, sizeof status) != 0 || strcmp(status, "mask 30,31\n") != 0) {
        printf("status: [%s]\n", status);
        return fail("status lists the new mask");
    }
    ep = epoll_create1(0);
    ev.events = EPOLLIN;
    ev.data.u64 = 7;
    if (ep < 0 || epoll_ctl(ep, EPOLL_CTL_ADD, fd, &ev) != 0 || epoll_wait(ep, &ev, 1, 0) != 0) {
        return fail("in a wait set, nothing pending");
    }
    if (kill(getpid(), SIGUSR2) != 0 || epoll_wait(ep, &ev, 1, 1000) != 1 || ev.data.u64 != 7) {
        return fail("the wait set reports the pending signal");
    }
    if (read(fd, &si, sizeof si) != (ssize_t)sizeof si || si.ssi_signo != SIGUSR2 || usr2_handled != 1) {
        return fail("the read takes SIGUSR2 instead of its handler");
    }
    close(ep);
    close(fd);
    printf("[ OK ] sigfd\n");
    return 0;
}
