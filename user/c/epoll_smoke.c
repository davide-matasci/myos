/* epoll-smoke: boot-CI guest test for the wait sets (/dev/wait) behind
 * epoll(7) (kernel/src/evfd.rs, libgloss evfd.c, docs/events.md).
 *
 * A set reports its ready members with their cookies: a pipe's write end
 * as writable, its read end once a byte is in it and not before; a
 * timeout that elapses; a forked child's write ending a wait (not before);
 * an event counter among the members; EPOLLET reporting a readiness once;
 * EPOLLONESHOT once until EPOLL_CTL_MOD; EEXIST, ENOENT, EBADF and ELOOP;
 * a member gone with its last fd; a set in a set (readable with it); the
 * set shared with a fork (a member the child adds). Prints [ OK ] epoll.
 */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define DELAY_MS 300

static int fail(const char *what) {
    printf("[ FAIL ] epoll %s (errno %d)\n", what, errno);
    return 1;
}

static long now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1000L + ts.tv_nsec / 1000000L;
}

static int add(int ep, int fd, uint32_t events, uint64_t cookie) {
    struct epoll_event ev;
    ev.events = events;
    ev.data.u64 = cookie;
    return epoll_ctl(ep, EPOLL_CTL_ADD, fd, &ev);
}

static int mod(int ep, int fd, uint32_t events, uint64_t cookie) {
    struct epoll_event ev;
    ev.events = events;
    ev.data.u64 = cookie;
    return epoll_ctl(ep, EPOLL_CTL_MOD, fd, &ev);
}

/* The one record epoll_wait gives in `timeout`, or -1. */
static int one(int ep, int timeout, struct epoll_event *ev) {
    int n = epoll_wait(ep, ev, 1, timeout);
    return n == 1 ? 0 : -1;
}

/* The members of the set behind `ep`, from its status. */
static int members(int ep) {
    char link[32], path[64], status[256];
    ssize_t n;
    int fd, count = -1;
    snprintf(link, sizeof link, "/proc/self/fd/%d", ep);
    n = readlink(link, path, sizeof path - 8);
    if (n <= 5) {
        return -1;
    }
    path[n] = '\0';
    strcpy(path + n - 5, "/status");
    fd = open(path, O_RDONLY);
    if (fd < 0) {
        return -1;
    }
    n = read(fd, status, sizeof status - 1);
    close(fd);
    if (n <= 0) {
        return -1;
    }
    status[n] = '\0';
    sscanf(status, "members %d", &count);
    return count;
}

int main(void) {
    struct epoll_event ev[4];
    int p[2], q[2], ep, ep2, efd, n, status = 0;
    char c = 'x';
    long t0;
    pid_t pid;
    eventfd_t v;

    ep = epoll_create1(EPOLL_CLOEXEC);
    if (ep < 0) {
        return fail("epoll_create1");
    }
    if (pipe(p) != 0 || pipe(q) != 0) {
        return fail("pipe");
    }
    if (add(ep, p[0], EPOLLIN, 10) != 0 || add(ep, p[1], EPOLLOUT, 11) != 0) {
        return fail("add");
    }
    if (members(ep) != 2) {
        return fail("status counts the members");
    }
    if (add(ep, p[0], EPOLLIN, 10) != -1 || errno != EEXIST) {
        return fail("a second add is EEXIST");
    }
    if (mod(ep, q[0], EPOLLIN, 1) != -1 || errno != ENOENT) {
        return fail("mod of a non-member is ENOENT");
    }
    if (epoll_ctl(ep, EPOLL_CTL_DEL, 63, NULL) != -1 || errno != EBADF) {
        return fail("a closed fd is EBADF");
    }
    if (add(ep, ep, EPOLLIN, 0) != -1 || errno != ELOOP) {
        return fail("a set in itself is ELOOP");
    }
    /* Only the write end is ready. */
    n = epoll_wait(ep, ev, 4, 0);
    if (n != 1 || ev[0].data.u64 != 11 || ev[0].events != EPOLLOUT) {
        return fail("an empty pipe: its write end only");
    }
    /* A 100 ms timeout, with the write end taken out. */
    if (epoll_ctl(ep, EPOLL_CTL_DEL, p[1], NULL) != 0) {
        return fail("del");
    }
    t0 = now_ms();
    if (epoll_wait(ep, ev, 4, 100) != 0 || now_ms() - t0 < 80) {
        return fail("a timeout elapses");
    }
    /* A byte makes the read end readable; a child's byte ends a wait. */
    if (write(p[1], &c, 1) != 1 || one(ep, 0, ev) != 0 || ev[0].data.u64 != 10 || ev[0].events != EPOLLIN) {
        return fail("a byte makes the read end readable");
    }
    if (read(p[0], &c, 1) != 1 || epoll_wait(ep, ev, 4, 0) != 0) {
        return fail("not readable once read");
    }
    t0 = now_ms();
    pid = fork();
    if (pid == 0) {
        usleep(DELAY_MS * 1000);
        _exit(write(p[1], &c, 1) == 1 ? 0 : 1);
    }
    if (one(ep, -1, ev) != 0 || ev[0].data.u64 != 10 || now_ms() - t0 < DELAY_MS / 2) {
        return fail("a child's write ends the wait, not before");
    }
    if (read(p[0], &c, 1) != 1 || waitpid(pid, &status, 0) != pid || status != 0) {
        return fail("child");
    }
    /* An event counter as a member. */
    efd = eventfd(0, 0);
    if (efd < 0 || add(ep, efd, EPOLLIN, 20) != 0 || epoll_wait(ep, ev, 4, 0) != 0) {
        return fail("an empty event counter is not ready");
    }
    if (eventfd_write(efd, 1) != 0 || one(ep, 1000, ev) != 0 || ev[0].data.u64 != 20) {
        return fail("a written event counter is ready");
    }
    if (eventfd_read(efd, &v) != 0 || v != 1) {
        return fail("eventfd read");
    }
    /* Edge-triggered: one report while the data stays. */
    if (add(ep, q[0], EPOLLIN | EPOLLET, 30) != 0 || write(q[1], "ab", 2) != 2) {
        return fail("et add");
    }
    if (one(ep, 0, ev) != 0 || ev[0].data.u64 != 30) {
        return fail("et: reported once");
    }
    if (epoll_wait(ep, ev, 4, 0) != 0) {
        return fail("et: not reported again while the data stays");
    }
    if (read(q[0], &c, 1) != 1 || read(q[0], &c, 1) != 1 || epoll_wait(ep, ev, 4, 0) != 0) {
        return fail("et: drained");
    }
    if (write(q[1], &c, 1) != 1 || one(ep, 0, ev) != 0 || ev[0].data.u64 != 30 || read(q[0], &c, 1) != 1) {
        return fail("et: reported again after a drain");
    }
    /* One shot: once, until re-armed. */
    if (mod(ep, q[0], EPOLLIN | EPOLLONESHOT, 31) != 0 || write(q[1], &c, 1) != 1) {
        return fail("oneshot mod");
    }
    if (one(ep, 0, ev) != 0 || ev[0].data.u64 != 31 || epoll_wait(ep, ev, 4, 0) != 0) {
        return fail("oneshot: once");
    }
    if (mod(ep, q[0], EPOLLIN, 32) != 0 || one(ep, 0, ev) != 0 || ev[0].data.u64 != 32 || read(q[0], &c, 1) != 1) {
        return fail("oneshot: re-armed by mod");
    }
    /* A member goes with its last fd. */
    close(efd);
    if (members(ep) != 3) {
        return fail("a closed member is forgotten");
    }
    if (epoll_ctl(ep, EPOLL_CTL_DEL, efd, NULL) != -1 || errno != EBADF) {
        return fail("del of the closed fd is EBADF");
    }
    /* A set in a set: readable with it. */
    ep2 = epoll_create(1);
    if (ep2 < 0 || add(ep2, ep, EPOLLIN, 40) != 0 || epoll_wait(ep2, ev, 4, 0) != 0) {
        return fail("a set with nothing ready is not readable");
    }
    if (add(ep, ep2, EPOLLIN, 41) != -1 || errno != ELOOP) {
        return fail("a cycle of sets is ELOOP");
    }
    if (write(p[1], &c, 1) != 1 || one(ep2, 1000, ev) != 0 || ev[0].data.u64 != 40) {
        return fail("the outer set sees the inner one ready");
    }
    if (one(ep, 0, ev) != 0 || ev[0].data.u64 != 10 || read(p[0], &c, 1) != 1) {
        return fail("the inner set still has its record");
    }
    /* The set is shared with a fork: the child's add shows in the parent. */
    pid = fork();
    if (pid == 0) {
        _exit(add(ep, q[1], EPOLLOUT, 50) == 0 ? 0 : 1);
    }
    if (waitpid(pid, &status, 0) != pid || status != 0 || members(ep) != 4) {
        return fail("a child's add is in the shared set");
    }
    if (one(ep, 0, ev) != 0 || ev[0].data.u64 != 50 || ev[0].events != EPOLLOUT) {
        return fail("the child's member is reported");
    }
    close(ep2);
    close(ep);
    printf("[ OK ] epoll\n");
    return 0;
}
