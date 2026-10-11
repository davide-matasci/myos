/* myos libgloss: eventfd(2), timerfd(2) and epoll(7) over the kernel's
 * event counters (/dev/event), timers (/dev/timer) and wait sets
 * (/dev/wait), docs/events.md.
 *
 * Reading /dev/event/clone (or /dev/timer/clone, /dev/wait/clone) makes an
 * object and answers its number N; the fd a program holds is N/data, read
 * and written by the kernel as the Linux fd is. What Linux does in the
 * eventfd/timerfd/epoll calls themselves goes to N/ctl as text
 * (`semaphore`, `arm FIRST [INTERVAL] [abs] [realtime]`, `disarm`, `add FD
 * EVENTS COOKIE`, ...) and comes back from N/status; the fd's
 * /proc/self/fd link names the directory. A timerfd's clock is remembered
 * here, per fd: it matters only to TFD_TIMER_ABSTIME, and is forgotten by
 * exec. */
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdlib.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <sys/timerfd.h>
#include <unistd.h>

#include "myos_syscalls.h"

/* /dev/event/N/status is the longest member path. */
#define EVFD_PATH 48

void myos_fd_nonblock_set(int fd, int on);

static void put_dec(char *out, unsigned long long v) {
    char tmp[21];
    int n = 0;
    do {
        tmp[n++] = (char)('0' + v % 10);
        v /= 10;
    } while (v != 0);
    while (n != 0) {
        *out++ = tmp[--n];
    }
    *out = '\0';
}

/* Read a whole small file (`status`, `clone`) into `buf`: its length, or -1. */
static int read_file(const char *path, char *buf, size_t cap) {
    int fd = open(path, O_RDONLY);
    size_t have = 0;
    if (fd < 0) {
        return -1;
    }
    while (have < cap - 1) {
        ssize_t n = read(fd, buf + have, cap - 1 - have);
        if (n <= 0) {
            break;
        }
        have += (size_t)n;
    }
    close(fd);
    buf[have] = '\0';
    return (int)have;
}

/* One line of text to `path`: 0, or -1 (errno from the open or write). */
static int write_file(const char *path, const char *text) {
    int fd = open(path, O_WRONLY);
    ssize_t n;
    if (fd < 0) {
        return -1;
    }
    n = write(fd, text, strlen(text));
    close(fd);
    return n < 0 ? -1 : 0;
}

/* A new object of `kind` ("event", "timer"): its `data` opened O_RDWR, the
 * O_CLOEXEC / O_NONBLOCK of `flags` applied, and its directory in `dir`. */
static int clone_open(const char *kind, int flags, char *dir) {
    char path[EVFD_PATH];
    char id[24];
    int n, fd;
    strcpy(path, "/dev/");
    strcat(path, kind);
    strcat(path, "/clone");
    n = read_file(path, id, sizeof id);
    if (n <= 0 || id[0] < '0' || id[0] > '9') {
        errno = n < 0 ? errno : EIO;
        return -1;
    }
    if (id[n - 1] == '\n') {
        id[n - 1] = '\0';
    }
    strcpy(dir, "/dev/");
    strcat(dir, kind);
    strcat(dir, "/");
    strcat(dir, id);
    strcpy(path, dir);
    strcat(path, "/data");
    fd = open(path, O_RDWR | (flags & O_CLOEXEC));
    if (fd < 0) {
        return -1;
    }
    if (flags & O_NONBLOCK) {
        myos_fd_nonblock_set(fd, 1);
    }
    return fd;
}

/* The directory of the object fd `fd` is open on under /dev/<kind>/, from
 * its /proc/self/fd link; -1 (EBADF, EINVAL) when it is no such fd. */
static int fd_dir(int fd, const char *kind, char *dir) {
    char link[32] = "/proc/self/fd/";
    char target[EVFD_PATH];
    char prefix[16];
    size_t plen;
    ssize_t n;
    char *end;
    if (fd < 0) {
        errno = EBADF;
        return -1;
    }
    put_dec(link + 14, (unsigned long long)fd);
    n = readlink(link, target, sizeof target - 1);
    if (n < 0) {
        errno = EBADF;
        return -1;
    }
    target[n] = '\0';
    strcpy(prefix, "/dev/");
    strcat(prefix, kind);
    strcat(prefix, "/");
    plen = strlen(prefix);
    end = strrchr(target, '/');
    if (strncmp(target, prefix, plen) != 0 || end == NULL || end == target + plen - 1
        || strcmp(end, "/data") != 0) {
        errno = EINVAL;
        return -1;
    }
    *end = '\0';
    strcpy(dir, target);
    return 0;
}

/* ---- eventfd ---------------------------------------------------------- */

int eventfd(unsigned int initval, int flags) {
    char dir[EVFD_PATH];
    char path[EVFD_PATH];
    int fd;
    if (flags & ~(EFD_SEMAPHORE | EFD_CLOEXEC | EFD_NONBLOCK)) {
        errno = EINVAL;
        return -1;
    }
    fd = clone_open("event", flags, dir);
    if (fd < 0) {
        return -1;
    }
    if (flags & EFD_SEMAPHORE) {
        strcpy(path, dir);
        strcat(path, "/ctl");
        if (write_file(path, "semaphore") != 0) {
            goto fail;
        }
    }
    if (initval != 0 && eventfd_write(fd, initval) != 0) {
        goto fail;
    }
    return fd;
fail:
    close(fd);
    return -1;
}

int eventfd_read(int fd, eventfd_t *value) {
    return read(fd, value, sizeof *value) == (ssize_t)sizeof *value ? 0 : -1;
}

int eventfd_write(int fd, eventfd_t value) {
    return write(fd, &value, sizeof value) == (ssize_t)sizeof value ? 0 : -1;
}

/* ---- timerfd ---------------------------------------------------------- */

/* The timerfds on CLOCK_REALTIME, a bit per fd. */
static unsigned long long tfd_realtime;

int timerfd_create(clockid_t clockid, int flags) {
    char dir[EVFD_PATH];
    int fd;
    if (clockid != CLOCK_REALTIME && clockid != CLOCK_MONOTONIC) {
        errno = EINVAL;
        return -1;
    }
    if (flags & ~(TFD_CLOEXEC | TFD_NONBLOCK)) {
        errno = EINVAL;
        return -1;
    }
    fd = clone_open("timer", flags, dir);
    if (fd >= 0 && fd < 64) {
        if (clockid == CLOCK_REALTIME) {
            tfd_realtime |= 1ull << fd;
        } else {
            tfd_realtime &= ~(1ull << fd);
        }
    }
    return fd;
}

static int timespec_ok(const struct timespec *ts) {
    return ts->tv_sec >= 0 && ts->tv_nsec >= 0 && ts->tv_nsec < 1000000000L;
}

static unsigned long long timespec_ns(const struct timespec *ts) {
    return (unsigned long long)ts->tv_sec * 1000000000ull + (unsigned long long)ts->tv_nsec;
}

static void ns_timespec(unsigned long long ns, struct timespec *ts) {
    ts->tv_sec = (time_t)(ns / 1000000000ull);
    ts->tv_nsec = (long)(ns % 1000000000ull);
}

int timerfd_gettime(int fd, struct itimerspec *value) {
    char path[EVFD_PATH];
    char status[64];
    unsigned long long left = 0, interval = 0;
    if (fd_dir(fd, "timer", path) != 0) {
        return -1;
    }
    strcat(path, "/status");
    if (read_file(path, status, sizeof status) < 0) {
        return -1;
    }
    if (strncmp(status, "armed ", 6) == 0) {
        char *end;
        left = strtoull(status + 6, &end, 10);
        interval = strtoull(end, NULL, 10);
    }
    ns_timespec(left, &value->it_value);
    ns_timespec(interval, &value->it_interval);
    return 0;
}

int timerfd_settime(int fd, int flags, const struct itimerspec *value, struct itimerspec *ovalue) {
    char path[EVFD_PATH];
    char cmd[80];
    if (value == NULL || !timespec_ok(&value->it_value) || !timespec_ok(&value->it_interval)
        || (flags & ~TFD_TIMER_ABSTIME)) {
        errno = EINVAL;
        return -1;
    }
    if (ovalue != NULL && timerfd_gettime(fd, ovalue) != 0) {
        return -1;
    }
    if (fd_dir(fd, "timer", path) != 0) {
        return -1;
    }
    strcat(path, "/ctl");
    if (value->it_value.tv_sec == 0 && value->it_value.tv_nsec == 0) {
        strcpy(cmd, "disarm");
    } else {
        strcpy(cmd, "arm ");
        put_dec(cmd + strlen(cmd), timespec_ns(&value->it_value));
        strcat(cmd, " ");
        put_dec(cmd + strlen(cmd), timespec_ns(&value->it_interval));
        if (flags & TFD_TIMER_ABSTIME) {
            strcat(cmd, " abs");
            if (fd < 64 && (tfd_realtime & (1ull << fd))) {
                strcat(cmd, " realtime");
            }
        }
    }
    return write_file(path, cmd);
}

/* ---- epoll ------------------------------------------------------------ */

/* The records a read of a wait set gives: {u64 cookie, u32 events, u32 0}. */
#define WAIT_RECORD 16
/* epoll_wait takes this many at most in one call (the rest stay ready). */
#define WAIT_BATCH 64

int epoll_create1(int flags) {
    char dir[EVFD_PATH];
    int fd;
    if (flags & ~EPOLL_CLOEXEC) {
        errno = EINVAL;
        return -1;
    }
    fd = clone_open("wait", flags, dir);
    if (fd >= 0) {
        /* epoll_wait polls for readiness, then reads what is ready: the
         * read must not block should it have gone away in between. */
        myos_fd_nonblock_set(fd, 1);
    }
    return fd;
}

int epoll_create(int size) {
    if (size <= 0) {
        errno = EINVAL;
        return -1;
    }
    return epoll_create1(0);
}

/* Whether the set at `dir` lists `fd` as a member (its status). */
static int epoll_has(const char *dir, int fd) {
    char path[EVFD_PATH];
    static char status[4096];
    char *line;
    int n;
    strcpy(path, dir);
    strcat(path, "/status");
    n = read_file(path, status, sizeof status);
    if (n <= 0) {
        return 0;
    }
    line = strchr(status, '\n');
    while (line != NULL && *++line != '\0') {
        if (strtol(line, NULL, 10) == fd && line[strcspn(line, " \n")] == ' ') {
            return 1;
        }
        line = strchr(line, '\n');
    }
    return 0;
}

int epoll_ctl(int epfd, int op, int fd, struct epoll_event *event) {
    char dir[EVFD_PATH];
    char path[EVFD_PATH];
    char cmd[96];
    int member;
    if (fd_dir(epfd, "wait", dir) != 0) {
        return -1;
    }
    if (fcntl(fd, F_GETFD) < 0) {
        errno = EBADF;
        return -1;
    }
    switch (op) {
    case EPOLL_CTL_ADD:
    case EPOLL_CTL_MOD:
        if (event == NULL) {
            errno = EFAULT;
            return -1;
        }
        strcpy(cmd, op == EPOLL_CTL_ADD ? "add " : "mod ");
        put_dec(cmd + strlen(cmd), (unsigned long long)fd);
        strcat(cmd, " ");
        if (!(event->events & (EPOLLIN | EPOLLOUT | EPOLLET | EPOLLONESHOT))) {
            strcat(cmd, "none");
        } else {
            const char *sep = "";
            if (event->events & EPOLLIN) { strcat(cmd, "in"); sep = ","; }
            if (event->events & EPOLLOUT) { strcat(cmd, sep); strcat(cmd, "out"); sep = ","; }
            if (event->events & EPOLLET) { strcat(cmd, sep); strcat(cmd, "et"); sep = ","; }
            if (event->events & EPOLLONESHOT) { strcat(cmd, sep); strcat(cmd, "oneshot"); }
        }
        strcat(cmd, " ");
        put_dec(cmd + strlen(cmd), event->data.u64);
        break;
    case EPOLL_CTL_DEL:
        strcpy(cmd, "del ");
        put_dec(cmd + strlen(cmd), (unsigned long long)fd);
        break;
    default:
        errno = EINVAL;
        return -1;
    }
    strcpy(path, dir);
    strcat(path, "/ctl");
    if (write_file(path, cmd) == 0) {
        return 0;
    }
    /* The kernel refuses without a reason: Linux's is worked out here. A
     * refused add of a set is a set in itself or a cycle of sets. */
    member = epoll_has(dir, fd);
    if (op == EPOLL_CTL_ADD) {
        errno = member ? EEXIST : fd_dir(fd, "wait", path) == 0 ? ELOOP : EINVAL;
    } else {
        errno = member ? EINVAL : ENOENT;
    }
    return -1;
}

int epoll_wait(int epfd, struct epoll_event *events, int maxevents, int timeout) {
    unsigned char buf[WAIT_BATCH * WAIT_RECORD];
    struct pollfd p = {epfd, POLLIN, 0};
    ssize_t n;
    int r, i, count;
    if (maxevents <= 0 || events == NULL) {
        errno = maxevents <= 0 ? EINVAL : EFAULT;
        return -1;
    }
    if (maxevents > WAIT_BATCH) {
        maxevents = WAIT_BATCH;
    }
    r = poll(&p, 1, timeout);
    if (r < 0) {
        return -1;
    }
    if (r == 0) {
        return 0;
    }
    if (p.revents & POLLNVAL) {
        errno = EBADF;
        return -1;
    }
    n = read(epfd, buf, (size_t)maxevents * WAIT_RECORD);
    if (n < 0) {
        /* Ready a moment ago, not any more: nothing this time. */
        return errno == EAGAIN ? 0 : -1;
    }
    count = (int)(n / WAIT_RECORD);
    for (i = 0; i < count; i++) {
        const unsigned char *rec = buf + i * WAIT_RECORD;
        memcpy(&events[i].data.u64, rec, 8);
        memcpy(&events[i].events, rec + 8, 4);
    }
    return count;
}

int epoll_pwait(int epfd, struct epoll_event *events, int maxevents, int timeout, const sigset_t *sigmask) {
    sigset_t old;
    int r, saved;
    if (sigmask != NULL && sigprocmask(SIG_SETMASK, sigmask, &old) != 0) {
        return -1;
    }
    r = epoll_wait(epfd, events, maxevents, timeout);
    if (sigmask != NULL) {
        saved = errno;
        sigprocmask(SIG_SETMASK, &old, NULL);
        errno = saved;
    }
    return r;
}
