/*
 * Boot smoke for the optional Linux compatibility layer. Built as a static-PIE
 * Linux binary against musl (not myos newlib) for each arch and run as
 * `linux linux-smoke`. Prints "LINUX-SMOKE OK" when every check passes.
 *
 * No printf: musl's printf needs the compiler runtime's quad-float helpers
 * on aarch64/riscv64, which this build does not ship.
 */
#define _GNU_SOURCE 1 /* memfd_create */
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <pthread.h>
#include <signal.h>
#include <spawn.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <sys/file.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/prctl.h>
#include <sys/reboot.h>
#include <sys/resource.h>
#include <sys/select.h>
#include <sys/signalfd.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <sys/syscall.h>
#include <sys/sysinfo.h>
#include <sys/timerfd.h>
#include <sys/un.h>
#include <sys/utsname.h>
#include <sys/wait.h>
#include <termios.h>
#include <time.h>
#include <unistd.h>

static int failures;

static void out(const char *s) {
    write(1, s, strlen(s));
}

static void check(int ok, const char *what) {
    out(ok ? "ok: " : "FAIL: ");
    out(what);
    out("\n");
    if (!ok) {
        failures++;
    }
}

static volatile sig_atomic_t got_sig, got_info_sig, got_uc;

static void on_sig(int sig) {
    got_sig = sig;
}

static void on_siginfo(int sig, siginfo_t *info, void *uc) {
    got_sig = sig;
    got_info_sig = info->si_signo;
    got_uc = uc != NULL;
}

static void handle(int sig, int flags) {
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = on_sig;
    sa.sa_flags = flags;
    sigaction(sig, &sa, NULL);
}

/* A child that signals this process after a short delay, then writes one
 * byte into the pipe. Returns its pid. */
static pid_t poke_later(int sig, int wfd) {
    pid_t c = fork();
    if (c == 0) {
        usleep(50000);
        kill(getppid(), sig);
        usleep(50000);
        write(wfd, "x", 1);
        _exit(0);
    }
    return c;
}

/* Blocking read interrupted by a caught signal: EINTR without SA_RESTART,
 * a transparent restart with it. */
static void check_interrupted_read(int restart) {
    int p[2];
    char ch = 0;
    int status;
    if (pipe(p) != 0) {
        check(0, "pipe");
        return;
    }
    handle(SIGUSR1, restart ? SA_RESTART : 0);
    got_sig = 0;
    pid_t c = poke_later(SIGUSR1, p[1]);
    ssize_t n = read(p[0], &ch, 1);
    if (restart) {
        check(n == 1 && ch == 'x' && got_sig == SIGUSR1, "SA_RESTART read resumes after handler");
    } else {
        int e = errno;
        check(n == -1 && e == EINTR && got_sig == SIGUSR1, "read interrupted by handler: EINTR");
        n = read(p[0], &ch, 1);
        check(n == 1 && ch == 'x', "read after EINTR");
    }
    waitpid(c, &status, 0);
    close(p[0]);
    close(p[1]);
}

/* Threads: shared memory, a mutex and a condition variable, thread-local
 * storage, thread ids, join. */
#define NTHREADS 4
#define ROUNDS 1000
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t cond = PTHREAD_COND_INITIALIZER;
static int counter, arrived;
static __thread long tls_id = -1;
static long tids[NTHREADS];

static void *worker(void *arg) {
    tls_id = (long)arg;
    tids[tls_id] = syscall(SYS_gettid);
    for (int i = 0; i < ROUNDS; i++) {
        pthread_mutex_lock(&lock);
        counter++;
        pthread_mutex_unlock(&lock);
    }
    /* Wait for the others, so all run at once (an ended thread's id may be
     * reused). */
    pthread_mutex_lock(&lock);
    arrived++;
    pthread_cond_broadcast(&cond);
    while (arrived < NTHREADS) {
        pthread_cond_wait(&cond, &lock);
    }
    pthread_mutex_unlock(&lock);
    /* Each thread still sees its own thread-local value. */
    return (void *)(long)(tls_id == (long)arg);
}

static void *blocker(void *arg) {
    (void)arg;
    pthread_mutex_lock(&lock);
    for (;;) {
        pthread_cond_wait(&cond, &lock);
    }
}

static void *exiter(void *arg) {
    (void)arg;
    exit(9);
}

static void check_threads(void) {
    pthread_t t[NTHREADS];
    int ok = 1;
    for (long i = 0; i < NTHREADS; i++) {
        ok &= pthread_create(&t[i], NULL, worker, (void *)i) == 0;
    }
    pthread_mutex_lock(&lock);
    while (ok && arrived < NTHREADS) {
        pthread_cond_wait(&cond, &lock);
    }
    pthread_mutex_unlock(&lock);
    for (int i = 0; i < NTHREADS; i++) {
        void *r = NULL;
        ok &= pthread_join(t[i], &r) == 0 && r == (void *)1;
    }
    check(ok && counter == NTHREADS * ROUNDS && tls_id == -1, "threads: mutex, condvar, TLS, join");
    int ids = syscall(SYS_gettid) == getpid();
    for (int i = 0; i < NTHREADS; i++) {
        for (int j = 0; j < i; j++) {
            ids &= tids[i] != tids[j];
        }
        ids &= tids[i] > 0 && tids[i] != getpid();
    }
    check(ids, "thread ids");

    /* exit() from one thread ends the process while another one waits. */
    pid_t c = fork();
    if (c == 0) {
        pthread_t b, e;
        pthread_create(&b, NULL, blocker, NULL);
        pthread_create(&e, NULL, exiter, NULL);
        pthread_join(b, NULL);
        _exit(1);
    }
    int st = 0;
    check(c > 0 && waitpid(c, &st, 0) == c && WIFEXITED(st) && WEXITSTATUS(st) == 9,
          "exit from a thread ends the process");
}

/* What a toolchain (cargo, rustc and its allocator, libcurl, SQLite) asks
 * of the layer besides the basics. */
static void check_shared_memory(void) {
    const char *name = "/linux-smoke";
    int fd, status;
    char *p, buf[8] = {0};
    pid_t pid;

    shm_unlink(name);
    fd = shm_open(name, O_RDWR | O_CREAT | O_EXCL, 0600);
    check(fd >= 0 && ftruncate(fd, 4096) == 0, "shm_open");
    p = fd < 0 ? MAP_FAILED : mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    check(p != MAP_FAILED, "mmap MAP_SHARED of a segment");
    if (p != MAP_FAILED) {
        strcpy(p, "parent");
        pid = fork();
        if (pid == 0) {
            int cfd = shm_open(name, O_RDWR, 0);
            char *c = cfd < 0 ? MAP_FAILED : mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, cfd, 0);
            if (c == MAP_FAILED || strcmp(c, "parent") != 0) {
                _exit(1);
            }
            strcpy(c + 100, "child");
            _exit(0);
        }
        check(pid > 0 && waitpid(pid, &status, 0) == pid && status == 0 && strcmp(p + 100, "child") == 0,
              "a segment shared by name with a forked child");
        munmap(p, 4096);
    }
    check(shm_unlink(name) == 0 && shm_open(name, O_RDWR, 0) < 0 && errno == ENOENT, "shm_unlink");
    if (fd >= 0) {
        close(fd);
    }

    fd = memfd_create("linux-smoke", MFD_CLOEXEC);
    check(fd >= 0 && fcntl(fd, F_GETFD) == FD_CLOEXEC && ftruncate(fd, 4096) == 0, "memfd_create");
    p = fd < 0 ? MAP_FAILED : mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    check(p != MAP_FAILED, "mmap a memfd");
    if (p != MAP_FAILED) {
        strcpy(p, "memfd");
        check(pread(fd, buf, 6, 0) == 6 && strcmp(buf, "memfd") == 0, "read() of a memfd sees the mapping's store");
        munmap(p, 4096);
    }
    if (fd >= 0) {
        close(fd);
    }

    p = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS, -1, 0);
    check(p != MAP_FAILED && p[0] == 0, "mmap MAP_SHARED | MAP_ANONYMOUS");
    if (p != MAP_FAILED) {
        pid = fork();
        if (pid == 0) {
            strcpy(p, "anon");
            _exit(0);
        }
        check(pid > 0 && waitpid(pid, &status, 0) == pid && status == 0 && strcmp(p, "anon") == 0,
              "an anonymous shared mapping shared with a forked child");
        munmap(p, 4096);
    }
}

/* A `sockaddr_un` for `name` (a leading '@': abstract) and its length. */
static socklen_t un_addr(struct sockaddr_un *a, const char *name) {
    memset(a, 0, sizeof *a);
    a->sun_family = AF_UNIX;
    strcpy(a->sun_path, name);
    if (name[0] == '@') {
        a->sun_path[0] = '\0';
        return (socklen_t)(offsetof(struct sockaddr_un, sun_path) + strlen(name));
    }
    return (socklen_t)(offsetof(struct sockaddr_un, sun_path) + strlen(name) + 1);
}

static int un_listener(const char *name) {
    struct sockaddr_un a;
    socklen_t len = un_addr(&a, name);
    int fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    if (fd >= 0 && (bind(fd, (struct sockaddr *)&a, len) < 0 || listen(fd, 4) < 0)) {
        int e = errno;
        close(fd);
        errno = e;
        return -1;
    }
    return fd;
}

static int un_connect(const char *name) {
    struct sockaddr_un a;
    socklen_t len = un_addr(&a, name);
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd >= 0 && connect(fd, (struct sockaddr *)&a, len) < 0) {
        int e = errno;
        close(fd);
        errno = e;
        return -1;
    }
    return fd;
}

/* Whether `fd`'s name (`peer`: its peer's) is `name`. */
static int un_named(int fd, int peer, const char *name) {
    struct sockaddr_un a, want;
    socklen_t len = sizeof a, wlen = un_addr(&want, name);
    int r = peer ? getpeername(fd, (struct sockaddr *)&a, &len) : getsockname(fd, (struct sockaddr *)&a, &len);
    return r == 0 && len == wlen && memcmp(&a, &want, wlen) == 0;
}

/* AF_UNIX listeners and clients (/net/unix): a second listener of a name
 * and a connect to an unknown one are refused, a nonblocking accept with
 * nothing queued says EAGAIN, poll sees a queued connection; a forked
 * client and the accepted socket exchange a greeting and see each other's
 * names; an abstract name works too. */
static void check_unix_sockets(void) {
    const char *name = "/tmp/.linux-smoke";
    int ls = un_listener(name);
    check(ls >= 0 && un_named(ls, 0, name), "AF_UNIX bind, listen, getsockname");
    check(un_listener(name) < 0 && errno == EADDRINUSE, "AF_UNIX listen: EADDRINUSE");
    check(un_connect("/tmp/.linux-smoke-none") < 0 && errno == ECONNREFUSED, "AF_UNIX connect: ECONNREFUSED");
    int fl = fcntl(ls, F_GETFL);
    check(fcntl(ls, F_SETFL, fl | O_NONBLOCK) == 0 && accept(ls, NULL, NULL) < 0 && errno == EAGAIN &&
              fcntl(ls, F_SETFL, fl) == 0,
          "AF_UNIX nonblocking accept: EAGAIN");

    pid_t pid = fork();
    if (pid == 0) {
        char b[5];
        int fd = un_connect(name);
        int ok = fd >= 0 && un_named(fd, 1, name) && write(fd, "hello", 5) == 5 && read(fd, b, 5) == 5 &&
                 memcmp(b, "world", 5) == 0;
        _exit(ok ? 0 : 1);
    }
    struct pollfd p = {.fd = ls, .events = POLLIN};
    struct sockaddr_un peer;
    socklen_t plen = sizeof peer;
    char b[5];
    check(poll(&p, 1, 10000) == 1 && (p.revents & POLLIN), "AF_UNIX poll: a connection is queued");
    int fd = accept4(ls, (struct sockaddr *)&peer, &plen, SOCK_CLOEXEC);
    check(fd >= 0 && plen == sizeof(sa_family_t) && (fcntl(fd, F_GETFD) & FD_CLOEXEC) && un_named(fd, 0, name),
          "AF_UNIX accept4: an unnamed peer, the listener's name");
    check(read(fd, b, 5) == 5 && memcmp(b, "hello", 5) == 0 && write(fd, "world", 5) == 5, "AF_UNIX exchange");
    int status = 0;
    check(pid > 0 && waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0,
          "AF_UNIX client");
    close(fd);
    close(ls);
    check((ls = un_listener(name)) >= 0, "AF_UNIX: a closed listener's name is free again");
    close(ls);

    ls = un_listener("@linux-smoke");
    fd = un_connect("@linux-smoke");
    int sfd = ls < 0 ? -1 : accept(ls, NULL, NULL);
    check(ls >= 0 && fd >= 0 && sfd >= 0 && un_named(ls, 0, "@linux-smoke") && un_named(fd, 1, "@linux-smoke") &&
              write(fd, "x", 1) == 1 && read(sfd, b, 1) == 1 && b[0] == 'x',
          "AF_UNIX abstract name");
    close(sfd);
    close(fd);
    close(ls);
}

/* select and pselect (musl: the select syscall on x86_64, pselect6 on
 * aarch64 and riscv64; pselect is pselect6 everywhere), as FLTK's event
 * loop waits: an empty pipe is not readable and a zero timeout returns at
 * once with the sets cleared, a short timeout waits it out, a pipe's write
 * end is writable, a byte makes the read end readable, a byte written by
 * a child later wakes a select without a timeout, a socket's readiness is
 * seen, and a closed fd is EBADF. */
static void check_select(void) {
    int p[2], sv[2];
    char c = 'x';
    fd_set r, w;
    struct timeval tv = {0, 0};
    check(pipe(p) == 0, "select: pipe");
    FD_ZERO(&r);
    FD_SET(p[0], &r);
    check(select(p[0] + 1, &r, NULL, NULL, &tv) == 0 && !FD_ISSET(p[0], &r), "select: empty pipe, zero timeout");
    struct timespec t0, t1, ts = {0, 50000000};
    FD_ZERO(&r);
    FD_SET(p[0], &r);
    clock_gettime(CLOCK_MONOTONIC, &t0);
    int n = pselect(p[0] + 1, &r, NULL, NULL, &ts, NULL);
    clock_gettime(CLOCK_MONOTONIC, &t1);
    long ms = (t1.tv_sec - t0.tv_sec) * 1000 + (t1.tv_nsec - t0.tv_nsec) / 1000000;
    check(n == 0 && ms >= 40, "pselect: a 50 ms timeout waits");
    FD_ZERO(&r);
    FD_ZERO(&w);
    FD_SET(p[0], &r);
    FD_SET(p[1], &w);
    check(select(p[1] + 1, &r, &w, NULL, &tv) == 1 && !FD_ISSET(p[0], &r) && FD_ISSET(p[1], &w),
          "select: a pipe's write end is writable");
    FD_ZERO(&r);
    FD_SET(p[0], &r);
    check(write(p[1], &c, 1) == 1 && select(p[0] + 1, &r, NULL, NULL, NULL) == 1 && FD_ISSET(p[0], &r) &&
              read(p[0], &c, 1) == 1,
          "select: a byte makes the read end readable");
    pid_t pid = fork();
    if (pid == 0) {
        usleep(100000);
        _exit(write(p[1], &c, 1) == 1 ? 0 : 1);
    }
    FD_ZERO(&r);
    FD_SET(p[0], &r);
    n = select(p[0] + 1, &r, NULL, NULL, NULL);
    int status = 0;
    check(n == 1 && FD_ISSET(p[0], &r) && read(p[0], &c, 1) == 1 && waitpid(pid, &status, 0) == pid &&
              WIFEXITED(status) && WEXITSTATUS(status) == 0,
          "select: a child's write wakes a select without a timeout");
    FD_ZERO(&r);
    check(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0 && write(sv[0], "s", 1) == 1, "select: socketpair");
    FD_SET(sv[1], &r);
    check(select(sv[1] + 1, &r, NULL, NULL, NULL) == 1 && FD_ISSET(sv[1], &r), "select: a socket is readable");
    close(sv[0]);
    close(sv[1]);
    close(p[1]);
    FD_ZERO(&r);
    FD_SET(p[1], &r);
    check(select(p[1] + 1, &r, NULL, NULL, &tv) == -1 && errno == EBADF, "select: a closed fd is EBADF");
    close(p[0]);
}

/* eventfd and timerfd (docs/events.md): a counter adds up its writes and a
 * read takes them (one at a time as a semaphore, from the initial value);
 * an empty nonblocking read is EAGAIN; a child's write wakes a blocked
 * read; a one-shot timer expires once after its delay, a periodic one
 * counts its expirations, gettime shows what is left, disarming stops it. */
static void check_evfd(void) {
    uint64_t v = 0;
    struct pollfd p;
    int fd = eventfd(0, EFD_NONBLOCK | EFD_CLOEXEC);
    check(fd >= 0, "eventfd");
    check(read(fd, &v, 8) == -1 && errno == EAGAIN, "eventfd: an empty nonblocking read is EAGAIN");
    v = 3;
    check(write(fd, &v, 8) == 8 && (v = 4, write(fd, &v, 8) == 8) && read(fd, &v, 8) == 8 && v == 7,
          "eventfd: a read takes the whole count");
    p.fd = fd;
    p.events = POLLIN | POLLOUT;
    check(poll(&p, 1, 0) == 1 && p.revents == POLLOUT, "eventfd: an empty counter is only writable");
    close(fd);
    fd = eventfd(2, EFD_SEMAPHORE);
    check(fd >= 0 && read(fd, &v, 8) == 8 && v == 1 && read(fd, &v, 8) == 8 && v == 1,
          "eventfd: a semaphore takes one per read, from the initial value");
    pid_t pid = fork();
    if (pid == 0) {
        usleep(100000);
        v = 1;
        _exit(write(fd, &v, 8) == 8 ? 0 : 1);
    }
    int status = 0;
    check(read(fd, &v, 8) == 8 && v == 1 && waitpid(pid, &status, 0) == pid && WIFEXITED(status) &&
              WEXITSTATUS(status) == 0,
          "eventfd: a child's write wakes a blocked read");
    close(fd);

    struct itimerspec its = {{0, 0}, {0, 50000000}}, cur;
    struct timespec t0, t1;
    fd = timerfd_create(CLOCK_MONOTONIC, TFD_CLOEXEC);
    check(fd >= 0, "timerfd_create");
    p.fd = fd;
    p.events = POLLIN;
    check(poll(&p, 1, 0) == 0, "timerfd: a disarmed timer is not readable");
    clock_gettime(CLOCK_MONOTONIC, &t0);
    check(timerfd_settime(fd, 0, &its, NULL) == 0 && timerfd_gettime(fd, &cur) == 0 && cur.it_value.tv_nsec > 0 &&
              cur.it_value.tv_nsec <= 50000000 && cur.it_interval.tv_nsec == 0,
          "timerfd: a one-shot armed 50 ms ahead");
    check(read(fd, &v, 8) == 8 && v == 1, "timerfd: a read waits for the expiry");
    clock_gettime(CLOCK_MONOTONIC, &t1);
    long ms = (t1.tv_sec - t0.tv_sec) * 1000 + (t1.tv_nsec - t0.tv_nsec) / 1000000;
    check(ms >= 40, "timerfd: not before 50 ms");
    check(poll(&p, 1, 0) == 0 && timerfd_gettime(fd, &cur) == 0 && cur.it_value.tv_nsec == 0,
          "timerfd: a one-shot is disarmed after its expiry");
    its.it_value.tv_nsec = 20000000;
    its.it_interval.tv_nsec = 20000000;
    check(timerfd_settime(fd, 0, &its, NULL) == 0, "timerfd: periodic");
    usleep(100000);
    check(read(fd, &v, 8) == 8 && v >= 3 && v <= 15, "timerfd: the expirations of 100 ms");
    memset(&its, 0, sizeof its);
    check(timerfd_settime(fd, 0, &its, &cur) == 0 && cur.it_interval.tv_nsec == 20000000,
          "timerfd: disarming returns the old setting");
    usleep(50000);
    check(poll(&p, 1, 0) == 0, "timerfd: disarmed");
    close(fd);
}

/* epoll (docs/events.md): a pipe's write end is reported writable, its
 * read end once a byte is in it; a timeout elapses; a child's write ends a
 * wait; EPOLLET reports once while the data stays; EPOLLONESHOT once until
 * EPOLL_CTL_MOD; EEXIST and ENOENT; a member goes with its last fd. */
static void check_epoll(void) {
    struct epoll_event ev[4];
    int p[2] = {-1, -1}, q[2] = {-1, -1}, ep, n;
    char c = 'x';
    ep = epoll_create1(EPOLL_CLOEXEC);
    check(ep >= 0 && pipe(p) == 0 && pipe(q) == 0, "epoll_create1");
    if (ep < 0 || p[0] < 0 || q[0] < 0) {
        return; /* the waits below would read whatever fd a missing pipe left */
    }
    ev[0].events = EPOLLIN;
    ev[0].data.u64 = 10;
    check(epoll_ctl(ep, EPOLL_CTL_ADD, p[0], &ev[0]) == 0, "epoll_ctl: add the read end");
    ev[0].events = EPOLLOUT;
    ev[0].data.u64 = 11;
    check(epoll_ctl(ep, EPOLL_CTL_ADD, p[1], &ev[0]) == 0, "epoll_ctl: add the write end");
    check(epoll_ctl(ep, EPOLL_CTL_ADD, p[1], &ev[0]) == -1 && errno == EEXIST, "epoll_ctl: a second add is EEXIST");
    check(epoll_ctl(ep, EPOLL_CTL_MOD, q[0], &ev[0]) == -1 && errno == ENOENT, "epoll_ctl: mod of a non-member is ENOENT");
    n = epoll_wait(ep, ev, 4, 0);
    check(n == 1 && ev[0].data.u64 == 11 && ev[0].events == EPOLLOUT, "epoll_wait: an empty pipe's write end only");
    check(epoll_ctl(ep, EPOLL_CTL_DEL, p[1], NULL) == 0, "epoll_ctl: del");
    struct timespec t0, t1;
    clock_gettime(CLOCK_MONOTONIC, &t0);
    n = epoll_wait(ep, ev, 4, 100);
    clock_gettime(CLOCK_MONOTONIC, &t1);
    long ms = (t1.tv_sec - t0.tv_sec) * 1000 + (t1.tv_nsec - t0.tv_nsec) / 1000000;
    check(n == 0 && ms >= 80, "epoll_wait: a 100 ms timeout elapses");
    check(write(p[1], &c, 1) == 1 && epoll_wait(ep, ev, 4, 1000) == 1 && ev[0].data.u64 == 10 && ev[0].events == EPOLLIN &&
              read(p[0], &c, 1) == 1 && epoll_wait(ep, ev, 4, 0) == 0,
          "epoll_wait: a byte makes the read end readable, a read takes it back");
    pid_t pid = fork();
    if (pid == 0) {
        usleep(100000);
        _exit(write(p[1], &c, 1) == 1 ? 0 : 1);
    }
    int status = 0;
    check(epoll_wait(ep, ev, 4, -1) == 1 && ev[0].data.u64 == 10 && read(p[0], &c, 1) == 1 &&
              waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0,
          "epoll_wait: a child's write ends a wait");
    ev[0].events = EPOLLIN | EPOLLET;
    ev[0].data.u64 = 30;
    check(epoll_ctl(ep, EPOLL_CTL_ADD, q[0], &ev[0]) == 0 && write(q[1], "ab", 2) == 2 && epoll_wait(ep, ev, 4, 0) == 1 &&
              ev[0].data.u64 == 30 && epoll_wait(ep, ev, 4, 0) == 0,
          "EPOLLET: reported once while the data stays");
    check(read(q[0], &c, 1) == 1 && read(q[0], &c, 1) == 1 && write(q[1], &c, 1) == 1 && epoll_wait(ep, ev, 4, 0) == 1 &&
              ev[0].data.u64 == 30 && read(q[0], &c, 1) == 1,
          "EPOLLET: reported again after a drain");
    ev[0].events = EPOLLIN | EPOLLONESHOT;
    ev[0].data.u64 = 31;
    check(epoll_ctl(ep, EPOLL_CTL_MOD, q[0], &ev[0]) == 0 && write(q[1], &c, 1) == 1 && epoll_wait(ep, ev, 4, 0) == 1 &&
              ev[0].data.u64 == 31 && epoll_wait(ep, ev, 4, 0) == 0,
          "EPOLLONESHOT: once");
    ev[0].events = EPOLLIN;
    ev[0].data.u64 = 32;
    check(epoll_ctl(ep, EPOLL_CTL_MOD, q[0], &ev[0]) == 0 && epoll_wait(ep, ev, 4, 0) == 1 && ev[0].data.u64 == 32 &&
              read(q[0], &c, 1) == 1,
          "EPOLLONESHOT: re-armed by mod");
    close(q[0]);
    check(epoll_ctl(ep, EPOLL_CTL_DEL, q[0], NULL) == -1 && errno == EBADF, "epoll: a closed member's fd is EBADF");
    close(q[1]);
    close(p[0]);
    close(p[1]);
    close(ep);
}

/* signalfd (docs/events.md): a blocked signal makes the fd readable and a
 * read takes it as a struct signalfd_siginfo; an empty nonblocking read is
 * EAGAIN; a child's kill ends a blocked read; the mask changes. */
static void check_signalfd(void) {
    struct signalfd_siginfo si;
    sigset_t set;
    struct pollfd p;
    sigemptyset(&set);
    sigaddset(&set, SIGUSR1);
    check(sigprocmask(SIG_BLOCK, &set, NULL) == 0, "signalfd: block SIGUSR1");
    int fd = signalfd(-1, &set, SFD_NONBLOCK | SFD_CLOEXEC);
    check(fd >= 0, "signalfd");
    p.fd = fd;
    p.events = POLLIN;
    check(poll(&p, 1, 0) == 0 && read(fd, &si, sizeof si) == -1 && errno == EAGAIN,
          "signalfd: nothing pending, EAGAIN");
    check(kill(getpid(), SIGUSR1) == 0 && poll(&p, 1, 1000) == 1 && (p.revents & POLLIN) &&
              read(fd, &si, sizeof si) == (ssize_t)sizeof si && si.ssi_signo == SIGUSR1 && poll(&p, 1, 0) == 0,
          "signalfd: a pending signal is read, not delivered");
    fcntl(fd, F_SETFL, 0);
    pid_t pid = fork();
    if (pid == 0) {
        usleep(100000);
        _exit(kill(getppid(), SIGUSR1) == 0 ? 0 : 1);
    }
    int status = 0;
    check(read(fd, &si, sizeof si) == (ssize_t)sizeof si && si.ssi_signo == SIGUSR1 &&
              waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0,
          "signalfd: a child's kill ends a blocked read");
    sigaddset(&set, SIGUSR2);
    check(sigprocmask(SIG_BLOCK, &set, NULL) == 0 && signalfd(fd, &set, 0) == fd && kill(getpid(), SIGUSR2) == 0 &&
              read(fd, &si, sizeof si) == (ssize_t)sizeof si && si.ssi_signo == SIGUSR2,
          "signalfd: the mask changes");
    sigprocmask(SIG_UNBLOCK, &set, NULL);
    close(fd);
}

/* `unix NAME`: connect to a server listening under NAME (a native one,
 * user/tests/kernel.sh; it may not listen yet: retried for 20 s), send a
 * line, read it back. */
static int unix_client(const char *name) {
    char b[16];
    int fd = -1;
    for (int i = 0; i < 200 && (fd = un_connect(name)) < 0; i++) {
        usleep(100000);
    }
    int ok = fd >= 0 && un_named(fd, 1, name) && write(fd, "linux-unix\n", 11) == 11;
    for (int got = 0, n; ok && got < 11; got += n) {
        n = read(fd, b + got, 11 - got);
        ok = n > 0;
    }
    ok = ok && memcmp(b, "linux-unix\n", 11) == 0;
    out(ok ? "LINUX-UNIX OK\n" : "LINUX-UNIX FAIL\n");
    return ok ? 0 : 1;
}

/* A thread setting fd flags while its process forks: the layer's fork hook
 * runs under the core's task lock and takes the fd tables' locks, which the
 * flag calls hold while asking the core for the pid; taken in the other
 * order that was a deadlock of two CPUs (both spinning, interrupts off). */
static volatile int flagging_done;

static void *flagger(void *arg) {
    int fd = *(int *)arg;
    while (!flagging_done) {
        fcntl(fd, F_SETFD, FD_CLOEXEC);
        fcntl(fd, F_SETFD, 0);
    }
    return NULL;
}

/* What Firefox asked for at startup: posix_fallocate (fallocate mode 0)
 * grows a file and other modes are EOPNOTSUPP; getresuid; getpriority;
 * prctl names a thread and refuses seccomp; mremap shrinks in place and
 * cannot grow; readahead is a hint. */
static void check_misc_calls(void) {
    int fd = open("/tmp/linux-smoke.falloc", O_RDWR | O_CREAT | O_TRUNC, 0644);
    struct stat st;
    check(fd >= 0 && posix_fallocate(fd, 0, 8192) == 0 && fstat(fd, &st) == 0 && st.st_size == 8192 &&
              posix_fallocate(fd, 0, 100) == 0 && fstat(fd, &st) == 0 && st.st_size == 8192,
          "posix_fallocate grows a file, never shrinks it");
    check(fd >= 0 && syscall(SYS_fallocate, fd, 1, 0, 100) == -1 && errno == EOPNOTSUPP,
          "fallocate FALLOC_FL_KEEP_SIZE: EOPNOTSUPP");
    check(fd >= 0 && syscall(SYS_readahead, fd, 0, 4096) == 0, "readahead");
    close(fd);
    unlink("/tmp/linux-smoke.falloc");
    uid_t r = 1, e = 1, s = 1;
    gid_t gr = 1, ge = 1, gs = 1;
    check(getresuid(&r, &e, &s) == 0 && r == 0 && e == 0 && s == 0 && getresgid(&gr, &ge, &gs) == 0 && gr == 0 &&
              ge == 0 && gs == 0,
          "getresuid, getresgid");
    errno = 0;
    check(getpriority(PRIO_PROCESS, 0) == 0 && errno == 0 && setpriority(PRIO_PROCESS, 0, 0) == 0,
          "getpriority, setpriority");
    check(prctl(PR_SET_NAME, "smoke") == 0 && prctl(PR_GET_SECCOMP) == -1 && errno == EINVAL,
          "prctl: a name, no seccomp");
    char *m = mmap(NULL, 3 * 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    check(m != MAP_FAILED && mremap(m, 3 * 4096, 4096, 0) == m && mremap(m, 4096, 8192, MREMAP_MAYMOVE) == MAP_FAILED &&
              errno == ENOMEM,
          "mremap: shrink in place, no growth");
    if (m != MAP_FAILED) {
        munmap(m, 4096);
        check(mremap(m, 4096, 8192, 0) == MAP_FAILED && errno == EFAULT, "mremap of a hole: EFAULT");
    }
    // musl measures the main thread's stack by growing one page after
    // another below it until mremap says EFAULT.
    pthread_attr_t a;
    void *low = NULL;
    size_t size = 0;
    char here;
    check(pthread_getattr_np(pthread_self(), &a) == 0 && pthread_attr_getstack(&a, &low, &size) == 0 &&
              (char *)low <= &here && &here < (char *)low + size,
          "pthread_getattr_np: the main thread's stack");
    struct sysinfo si;
    check(sysinfo(&si) == 0 && si.totalram > 0 && si.freeram <= si.totalram && si.mem_unit == 1, "sysinfo");
    // Firefox's IPC aborts on a socket that says it buffers nothing.
    int sv[2], snd = 0, rcv = 0;
    socklen_t l1 = sizeof snd, l2 = sizeof rcv;
    check(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0 && getsockopt(sv[0], SOL_SOCKET, SO_SNDBUF, &snd, &l1) == 0 &&
              getsockopt(sv[0], SOL_SOCKET, SO_RCVBUF, &rcv, &l2) == 0 && snd > 0 && rcv > 0 && l1 == sizeof snd,
          "getsockopt SO_SNDBUF, SO_RCVBUF");
    close(sv[0]);
    close(sv[1]);
    // Fds past 64 keep their flags (256 per process).
    struct rlimit nofile;
    int hi = dup3(1, 200, O_CLOEXEC);
    check(getrlimit(RLIMIT_NOFILE, &nofile) == 0 && nofile.rlim_cur == 256 && hi == 200 &&
              fcntl(hi, F_GETFD) == FD_CLOEXEC && fcntl(hi, F_SETFL, O_NONBLOCK) == 0 &&
              (fcntl(hi, F_GETFL) & O_NONBLOCK) != 0,
          "fd 200: RLIMIT_NOFILE, FD_CLOEXEC, O_NONBLOCK");
    close(hi);
}

static void check_fork_vs_fds(void) {
    int fd = open("/dev/null", O_RDONLY);
    pthread_t t;
    int ok = fd >= 0 && pthread_create(&t, NULL, flagger, &fd) == 0;
    for (int i = 0; ok && i < 100; i++) {
        pid_t c = fork();
        if (c == 0) {
            _exit(0);
        }
        int status = 0;
        ok = c > 0 && waitpid(c, &status, 0) == c && WIFEXITED(status) && WEXITSTATUS(status) == 0;
    }
    flagging_done = 1;
    if (fd >= 0) {
        pthread_join(t, NULL);
        close(fd);
    }
    check(ok, "fork while a thread sets fd flags");
}

static void check_toolchain_calls(char *self) {
    /* posix_spawn: musl's clone(CLONE_VM | CLONE_VFORK) on a stack of its own. */
    pid_t c;
    int status = 0;
    char *args[] = {self, "child", NULL};
    check(posix_spawn(&c, self, NULL, NULL, args, NULL) == 0 &&
              waitpid(c, &status, 0) == c && WIFEXITED(status) && WEXITSTATUS(status) == 5,
          "posix_spawn");

    /* The online CPUs, as cargo and rustc count them for their jobs. */
    unsigned long cpus[16] = {0};
    long got = syscall(SYS_sched_getaffinity, 0, sizeof cpus, cpus);
    int n = 0;
    for (int i = 0; got > 0 && i < got / 8; i++) {
        n += __builtin_popcountl(cpus[i]);
    }
    check(got > 0 && n >= 1 && n == sysconf(_SC_NPROCESSORS_ONLN), "sched_getaffinity");

    /* A `#!` script runs its interpreter (here this binary, as "child"); a
     * file that is neither fails the exec and leaves the caller as it was. */
    const char *script = "/tmp/linux-smoke.sh";
    int sfd = open(script, O_WRONLY | O_CREAT | O_TRUNC, 0755);
    if (sfd >= 0) {
        write(sfd, "#!", 2);
        write(sfd, self, strlen(self));
        write(sfd, " child\n", 7);
        close(sfd);
    }
    char *sargs[] = {(char *)script, NULL};
    check(sfd >= 0 && posix_spawn(&c, script, NULL, NULL, sargs, NULL) == 0 &&
              waitpid(c, &status, 0) == c && WIFEXITED(status) && WEXITSTATUS(status) == 5,
          "#! script");
    sfd = open(script, O_WRONLY | O_TRUNC);
    if (sfd >= 0) {
        write(sfd, "echo not an executable\n", 23);
        close(sfd);
    }
    check(sfd >= 0 && execv(script, sargs) == -1 && errno == ENOEXEC, "exec of a non-ELF: ENOEXEC");
    unlink(script);

    /* pipe2 flags: close-on-exec kept per fd, an empty non-blocking end. */
    int p[2];
    char ch;
    check(pipe2(p, O_CLOEXEC | O_NONBLOCK) == 0 && (fcntl(p[0], F_GETFD) & FD_CLOEXEC) &&
              read(p[0], &ch, 1) == -1 && errno == EAGAIN,
          "pipe2 O_CLOEXEC|O_NONBLOCK");
    close(p[0]);
    close(p[1]);

    uint64_t v = 1;
    int efd = eventfd(0, EFD_CLOEXEC);
    check(efd >= 0 && write(efd, &v, 8) == 8 && read(efd, &v, 8) == 8 && v >= 1, "eventfd");
    close(efd);

    int sv[2];
    char b[4] = {0};
    check(socketpair(AF_UNIX, SOCK_STREAM, 0, sv) == 0 && write(sv[0], "to", 2) == 2 &&
              read(sv[1], b, 2) == 2 && write(sv[1], "fro", 3) == 3 && read(sv[0], b, 3) == 3 &&
              memcmp(b, "fro", 3) == 0,
          "socketpair");
    close(sv[0]);
    close(sv[1]);
    check_unix_sockets();
    check_select();
    check_evfd();
    check_epoll();
    check_signalfd();

    /* A file cut, grown and written at a position, with locks and fsync. */
    struct stat st;
    int fd = open("/tmp/linux-smoke.trunc", O_RDWR | O_CREAT | O_TRUNC, 0644);
    check(fd >= 0 && write(fd, "hello", 5) == 5 && ftruncate(fd, 0) == 0 && fstat(fd, &st) == 0 &&
              st.st_size == 0 && ftruncate(fd, 100) == 0 && fstat(fd, &st) == 0 && st.st_size == 100,
          "ftruncate");
    check(pwrite(fd, "ab", 2, 10) == 2 && pread(fd, b, 2, 10) == 2 && memcmp(b, "ab", 2) == 0,
          "pwrite");
    check(flock(fd, LOCK_EX) == 0 && fsync(fd) == 0 && flock(fd, LOCK_UN) == 0, "flock, fsync");
    /* Locks are kept: another process meets them. */
    struct flock fl = {.l_type = F_WRLCK, .l_whence = SEEK_SET, .l_start = 0, .l_len = 10};
    check(flock(fd, LOCK_EX) == 0 && fcntl(fd, F_SETLK, &fl) == 0, "F_SETLK");
    pid_t pid = fork();
    if (pid == 0) {
        int mine = open("/tmp/linux-smoke.trunc", O_RDWR);
        struct flock want = {.l_type = F_RDLCK, .l_whence = SEEK_SET, .l_start = 5, .l_len = 1};
        struct flock ofd = {.l_type = F_WRLCK, .l_whence = SEEK_SET, .l_start = 9, .l_len = 1};
        struct flock free_ = {.l_type = F_WRLCK, .l_whence = SEEK_SET, .l_start = 10, .l_len = 0};
        int ok = flock(mine, LOCK_EX | LOCK_NB) < 0 && errno == EWOULDBLOCK &&
                 fcntl(mine, F_SETLK, &want) < 0 && errno == EAGAIN &&
                 fcntl(mine, F_GETLK, &want) == 0 && want.l_type == F_WRLCK && want.l_pid == getppid() &&
                 fcntl(mine, F_OFD_SETLK, &ofd) < 0 && errno == EAGAIN && fcntl(mine, F_SETLK, &free_) == 0;
        _exit(ok ? 0 : 1);
    }
    int lstatus = 0;
    check(pid > 0 && waitpid(pid, &lstatus, 0) == pid && WIFEXITED(lstatus) && WEXITSTATUS(lstatus) == 0,
          "flock and record locks are kept");
    close(fd);
    /* No hard links; O_NOFOLLOW and O_DIRECTORY are kept. */
    check(link("/tmp/linux-smoke.trunc", "/tmp/linux-smoke.link") < 0 && errno == EPERM, "link: EPERM");
    unlink("/tmp/linux-smoke.link");
    check(symlink("linux-smoke.trunc", "/tmp/linux-smoke.link") == 0 &&
              open("/tmp/linux-smoke.link", O_RDONLY | O_NOFOLLOW) < 0 && errno == ELOOP,
          "O_NOFOLLOW");
    check(open("/tmp/linux-smoke.trunc", O_RDONLY | O_DIRECTORY) < 0 && errno == ENOTDIR, "O_DIRECTORY");
    unlink("/tmp/linux-smoke.link");
    unlink("/tmp/linux-smoke.trunc");

    /* MADV_DONTNEED: the pages read as zero again (allocators count on it). */
    char *m = mmap(NULL, 8192, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (m != MAP_FAILED) {
        memset(m, 0x5a, 8192);
    }
    check(m != MAP_FAILED && madvise(m, 8192, MADV_DONTNEED) == 0 && m[0] == 0 && m[8191] == 0,
          "madvise MADV_DONTNEED");
    if (m != MAP_FAILED) {
        munmap(m, 8192);
    }
}

int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "child") == 0) {
        return 5;
    }
    /* `mtime FILE`: the file's modification time is a real date (after
     * 2001), as a filesystem that keeps times reports it. */
    if (argc > 2 && strcmp(argv[1], "mtime") == 0) {
        struct stat st;
        return stat(argv[2], &st) == 0 && st.st_mtime > 1000000000 ? 0 : 1;
    }
    if (argc > 2 && strcmp(argv[1], "unix") == 0) {
        return unix_client(argv[2]);
    }
    /* `enosys`: a syscall no arch has gets ENOSYS (with the layer's trace
     * on, user/tests/kernel.sh, it is also reported on the console). */
    if (argc > 1 && strcmp(argv[1], "enosys") == 0) {
        errno = 0;
        int ok = syscall(4000, 1, 2, 3) == -1 && errno == ENOSYS;
        out(ok ? "LINUX-ENOSYS OK\n" : "LINUX-ENOSYS FAIL\n");
        return !ok;
    }
    /* `evfd`: the event fds alone, for a run chrooted into a bare root
     * (`linux --root`), where /dev is a bind of the system's. */
    if (argc > 1 && strcmp(argv[1], "evfd") == 0) {
        check_evfd();
        check_epoll();
        check_signalfd();
        out(failures ? "LINUX-EVFD FAIL\n" : "LINUX-EVFD OK\n");
        return failures != 0;
    }

    struct utsname u;
    check(uname(&u) == 0 && strcmp(u.sysname, "Linux") == 0, "uname");

    /* The host name is the kernel's: sethostname sets what uname reports. */
    char host[65];
    strcpy(host, u.nodename);
    check(sethostname("lxhost", 6) == 0 && uname(&u) == 0 && strcmp(u.nodename, "lxhost") == 0
              && sethostname(host, strlen(host)) == 0,
          "sethostname");

    /* reboot(2): an init's Ctrl-Alt-Del setting is accepted (and does
     * nothing), an unknown command refused; nothing here takes the
     * system down. */
    check(reboot(RB_DISABLE_CAD) == 0 && reboot(0x12345678) == -1 && errno == EINVAL, "reboot");

    char cwd[256];
    check(getcwd(cwd, sizeof cwd) != NULL && cwd[0] == '/', "getcwd");

    /* stdio file write, raw read back, fstat. */
    const char *path = "/tmp/linux-smoke.txt";
    FILE *f = fopen(path, "w");
    check(f != NULL && fputs("hello linux\n", f) >= 0 && fclose(f) == 0, "fopen/fputs");
    char buf[64] = {0};
    int fd = open(path, O_RDONLY);
    struct stat st;
    check(fd >= 0 && read(fd, buf, sizeof buf) == 12 && strcmp(buf, "hello linux\n") == 0,
          "open/read");
    check(fd >= 0 && fstat(fd, &st) == 0 && S_ISREG(st.st_mode) && st.st_size == 12, "fstat");

    /* File times: a new tmpfs file's are real dates; utimensat sets both,
     * futimens one of them (UTIME_OMIT keeps the other). */
    check(st.st_mtime > 1000000000 && st.st_atime > 1000000000, "tmpfs times");
    struct timespec ts[2] = {{1000, 0}, {2000, 0}};
    check(utimensat(AT_FDCWD, path, ts, 0) == 0 && stat(path, &st) == 0 && st.st_atime == 1000
              && st.st_mtime == 2000,
          "utimensat");
    ts[0].tv_nsec = UTIME_OMIT;
    ts[1].tv_sec = 3000;
    check(fd >= 0 && futimens(fd, ts) == 0 && stat(path, &st) == 0 && st.st_atime == 1000
              && st.st_mtime == 3000,
          "futimens");

    /* statvfs and fstatvfs (musl calls statfs and fstatfs): /tmp's
     * filesystem has blocks, some free, and the fd's is the same one. */
    struct statvfs sv, fsv;
    check(statvfs("/tmp", &sv) == 0 && sv.f_frsize > 0 && sv.f_blocks > 0 && sv.f_bfree > 0
              && sv.f_bfree <= sv.f_blocks && sv.f_bavail <= sv.f_bfree,
          "statvfs");
    check(fd >= 0 && fstatvfs(fd, &fsv) == 0 && fsv.f_blocks == sv.f_blocks && fsv.f_frsize == sv.f_frsize,
          "fstatvfs");
    close(fd);

    /* readdir. */
    int found = 0;
    DIR *d = opendir("/tmp");
    if (d) {
        struct dirent *e;
        while ((e = readdir(d)) != NULL) {
            if (strcmp(e->d_name, "linux-smoke.txt") == 0) {
                found = 1;
            }
        }
        closedir(d);
    }
    check(found, "opendir/readdir");

    /* Large malloc: musl serves it with mmap (the native mmap window is
     * 1 MiB in total). */
    size_t sz = 256 << 10;
    char *big = malloc(sz);
    if (big) {
        memset(big, 0x5a, sz);
    }
    check(big != NULL && big[sz - 1] == 0x5a, "malloc 256K (mmap)");
    free(big);

    /* pipe + fork + exit status. */
    int p[2];
    int status = 0;
    if (pipe(p) == 0) {
        pid_t c = fork();
        if (c == 0) {
            close(p[0]);
            write(p[1], "ping", 4);
            _exit(7);
        }
        close(p[1]);
        char pb[8] = {0};
        int n = read(p[0], pb, sizeof pb);
        close(p[0]);
        check(c > 0 && n == 4 && memcmp(pb, "ping", 4) == 0, "pipe+fork");
        check(waitpid(c, &status, 0) == c && WIFEXITED(status) && WEXITSTATUS(status) == 7,
              "waitpid exit status");
    } else {
        check(0, "pipe");
    }

    /* fork + execve (this binary again, Linux personality kept). */
    pid_t c = fork();
    if (c == 0) {
        char *args[] = {argv[0], "child", NULL};
        execv("/proc/self/exe", args);
        execvp(argv[0], args);
        _exit(99);
    }
    check(waitpid(c, &status, 0) == c && WIFEXITED(status) && WEXITSTATUS(status) == 5,
          "fork+execve");

    /* Signal numbers are Linux ones: SIGUSR1 is 10 here, 30 natively. */
    c = fork();
    if (c == 0) {
        for (;;) {
            usleep(10000);
        }
    }
    usleep(20000);
    /* waitid: WNOHANG on a live child reports nobody; WNOWAIT reports the
     * death and leaves the child to waitpid. */
    siginfo_t si;
    memset(&si, 0xff, sizeof si);
    check(waitid(P_PID, c, &si, WEXITED | WNOHANG) == 0 && si.si_pid == 0, "waitid WNOHANG");
    check(kill(c, SIGUSR1) == 0, "kill");
    check(waitid(P_PID, c, &si, WEXITED | WNOWAIT) == 0 && si.si_signo == SIGCHLD &&
              si.si_code == CLD_KILLED && si.si_pid == c && si.si_status == SIGUSR1,
          "waitid WNOWAIT");
    check(waitpid(c, &status, 0) == c && WIFSIGNALED(status) && WTERMSIG(status) == SIGUSR1,
          "WTERMSIG(SIGUSR1)");

    /* Handlers: SA_SIGINFO handler run by raise(). */
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_sigaction = on_siginfo;
    sa.sa_flags = SA_SIGINFO;
    sigaction(SIGUSR1, &sa, NULL);
    got_sig = got_info_sig = got_uc = 0;
    raise(SIGUSR1);
    check(got_sig == SIGUSR1 && got_info_sig == SIGUSR1 && got_uc, "SA_SIGINFO handler");
    struct sigaction old;
    check(sigaction(SIGUSR1, NULL, &old) == 0 && old.sa_sigaction == on_siginfo &&
              (old.sa_flags & SA_SIGINFO),
          "sigaction reports the handler");

    /* Blocked: stays pending, runs on unblock. */
    sigset_t set, pend;
    sigemptyset(&set);
    sigaddset(&set, SIGUSR2);
    handle(SIGUSR2, 0);
    got_sig = 0;
    sigprocmask(SIG_BLOCK, &set, NULL);
    raise(SIGUSR2);
    sigpending(&pend);
    check(got_sig == 0 && sigismember(&pend, SIGUSR2), "blocked signal pending");
    sigprocmask(SIG_UNBLOCK, &set, NULL);
    check(got_sig == SIGUSR2, "handler runs on unblock");

    /* sigwait takes a blocked signal instead of running the handler. */
    got_sig = 0;
    sigprocmask(SIG_BLOCK, &set, NULL);
    raise(SIGUSR2);
    int taken = 0;
    check(sigwait(&set, &taken) == 0 && taken == SIGUSR2 && got_sig == 0, "sigwait");
    sigprocmask(SIG_UNBLOCK, &set, NULL);

    check_interrupted_read(0);
    check_interrupted_read(1);
    check_threads();

    check(unlink(path) == 0 && stat(path, &st) == -1 && errno == ENOENT, "unlink/ENOENT");

    /* Shared memory: a POSIX segment, a memfd and an anonymous MAP_SHARED
     * mapping, each shared with a forked child (docs/linux-compat.md). */
    check_shared_memory();

    /* The terminal: the boot test runs this with stdout in a file and stdin
     * on the console. musl's tty calls are the Linux ioctls, which the layer
     * serves from the terminal's ctl file (docs/tty.md). */
    struct termios t;
    struct winsize ws;
    char link[64];
    ssize_t ln = readlink("/proc/self/fd/0", link, sizeof link - 1);
    link[ln > 0 ? ln : 0] = '\0';
    check(isatty(0) == 1 && isatty(1) == 0, "isatty");
    check(tcgetattr(0, &t) == 0 && (t.c_lflag & ICANON) && t.c_cc[VINTR] == 3, "tcgetattr");
    check(ioctl(0, TIOCGWINSZ, &ws) == 0 && ws.ws_row > 0 && ws.ws_col > 0, "TIOCGWINSZ");
    check(ln > 5 && strcmp(link + ln - 5, "/data") == 0, "fd link names the terminal");
    check(readlink("/no/such/link", link, sizeof link) == -1 && errno == ENOENT, "readlink of nothing: ENOENT");
    check(tcgetattr(1, &t) == -1 && errno == ENOTTY, "tcgetattr on a file: ENOTTY");

    check_toolchain_calls(argv[0]);
    check_fork_vs_fds();
    check_misc_calls();

    if (failures) {
        out("LINUX-SMOKE FAIL\n");
        return 1;
    }
    out("LINUX-SMOKE OK\n");
    return 0;
}
