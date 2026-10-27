/*
 * Boot smoke for the optional Linux compatibility layer. Built as a static-PIE
 * Linux binary against musl (not myos newlib) for each arch and run as
 * `linux linux-smoke`. Prints "LINUX-SMOKE OK" when every check passes.
 *
 * No printf: musl's printf needs the compiler runtime's quad-float helpers
 * on aarch64/riscv64, which this build does not ship.
 */
#define _GNU_SOURCE 1 /* memfd_create */
#include <arpa/inet.h>
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <netdb.h>
#include <poll.h>
#include <pthread.h>
#include <setjmp.h>
#include <sched.h>
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
#include <ucontext.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <sys/syscall.h>
#include <sys/sysinfo.h>
#include <sys/timerfd.h>
#include <sys/uio.h>
#include <sys/un.h>
#include <sys/utsname.h>
#include <sys/wait.h>
#include <sys/xattr.h>
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
/* SCM_RIGHTS: send `data` (one byte) with the fds `fds`. */
static int send_fds(int sock, char data, const int *fds, int n) {
    union {
        char buf[CMSG_SPACE(4 * sizeof(int))];
        struct cmsghdr align;
    } u;
    struct iovec iov = {&data, 1};
    struct msghdr m;
    memset(&m, 0, sizeof m);
    m.msg_iov = &iov;
    m.msg_iovlen = 1;
    if (n > 0) {
        memset(&u, 0, sizeof u);
        m.msg_control = u.buf;
        m.msg_controllen = CMSG_SPACE(n * sizeof(int));
        struct cmsghdr *c = CMSG_FIRSTHDR(&m);
        c->cmsg_level = SOL_SOCKET;
        c->cmsg_type = SCM_RIGHTS;
        c->cmsg_len = CMSG_LEN(n * sizeof(int));
        memcpy(CMSG_DATA(c), fds, n * sizeof(int));
    }
    return sendmsg(sock, &m, 0) == 1 ? 0 : -1;
}

/* Receive up to `len` bytes and up to `room` fds (the control buffer's
 * room): the bytes, with the fds in `fds`, their count in `*n` and the
 * message flags in `*flags`. */
static int recv_fds(int sock, char *buf, int len, int *fds, int room, int *n, int *flags) {
    union {
        char buf[CMSG_SPACE(4 * sizeof(int))];
        struct cmsghdr align;
    } u;
    struct iovec iov = {buf, (size_t)len};
    struct msghdr m;
    memset(&m, 0, sizeof m);
    m.msg_iov = &iov;
    m.msg_iovlen = 1;
    m.msg_control = u.buf;
    /* Exactly `room` fds' worth: CMSG_SPACE would round up to room for more. */
    m.msg_controllen = room > 0 ? CMSG_LEN(room * sizeof(int)) : 0;
    if (room == 0) {
        m.msg_control = NULL;
    }
    int r = (int)recvmsg(sock, &m, 0);
    *n = 0;
    *flags = m.msg_flags;
    for (struct cmsghdr *c = CMSG_FIRSTHDR(&m); r >= 0 && c != NULL; c = CMSG_NXTHDR(&m, c)) {
        if (c->cmsg_level == SOL_SOCKET && c->cmsg_type == SCM_RIGHTS) {
            int k = (int)((c->cmsg_len - CMSG_LEN(0)) / sizeof(int));
            memcpy(fds + *n, CMSG_DATA(c), k * sizeof(int));
            *n += k;
        }
    }
    return r;
}

/* Seals on a memfd (glycin's image loaders seal the ones they pass back):
 * kept and reported, those of the memfd whichever fd names it (one
 * received over a unix socket too); F_SEAL_SEAL stops more; a file that
 * is no memfd has none (EINVAL). */
static const char *memfd_seals(void) {
    const int all = F_SEAL_GROW | F_SEAL_SHRINK | F_SEAL_WRITE | F_SEAL_SEAL;
    int fd = memfd_create("linux-smoke-seals", MFD_CLOEXEC | MFD_ALLOW_SEALING);
    int sv[2], got[4], n = 0, fl;
    char c;
    if (fd < 0) {
        return "seals: memfd_create(MFD_ALLOW_SEALING)";
    }
    if (fcntl(fd, F_GET_SEALS) != 0) {
        return "seals: none on a new memfd";
    }
    if (fcntl(fd, F_ADD_SEALS, F_SEAL_GROW) != 0 || fcntl(fd, F_GET_SEALS) != F_SEAL_GROW) {
        return "seals: F_ADD_SEALS kept";
    }
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) != 0 || send_fds(sv[0], 's', &fd, 1) != 0 ||
        recv_fds(sv[1], &c, 1, got, 4, &n, &fl) != 1 || n != 1) {
        return "seals: send a memfd";
    }
    if (fcntl(got[0], F_ADD_SEALS, F_SEAL_SHRINK | F_SEAL_WRITE | F_SEAL_SEAL) != 0) {
        return "seals: F_ADD_SEALS on the received memfd";
    }
    if (fcntl(got[0], F_GET_SEALS) != all || fcntl(fd, F_GET_SEALS) != all) {
        return "seals: the memfd's, whichever fd names it";
    }
    if (fcntl(fd, F_ADD_SEALS, F_SEAL_GROW) != -1 || errno != EPERM) {
        return "seals: F_SEAL_SEAL stops more";
    }
    close(got[0]);
    close(sv[0]);
    close(sv[1]);
    close(fd);
    fd = open("/tmp/linux-smoke-seals", O_RDWR | O_CREAT | O_TRUNC, 0600);
    if (fd < 0 || fcntl(fd, F_GET_SEALS) != -1 || errno != EINVAL) {
        return "seals: none on a file";
    }
    close(fd);
    unlink("/tmp/linux-smoke-seals");
    return NULL;
}

/* The sender's pid of what a read returns (SO_PASSCRED, SCM_CREDENTIALS):
 * how WebKit learns its web process's pid. A read never returns two
 * senders' bytes. */
static pid_t recv_cred(int sock) {
    char c, control[CMSG_SPACE(sizeof(struct ucred))];
    struct iovec iov = { &c, 1 };
    struct msghdr msg = { .msg_iov = &iov, .msg_iovlen = 1, .msg_control = control, .msg_controllen = sizeof control };
    if (recvmsg(sock, &msg, 0) != 1) {
        return -1;
    }
    struct cmsghdr *h = CMSG_FIRSTHDR(&msg);
    if (h == NULL || h->cmsg_level != SOL_SOCKET || h->cmsg_type != SCM_CREDENTIALS ||
        h->cmsg_len != CMSG_LEN(sizeof(struct ucred))) {
        return -1;
    }
    struct ucred cred;
    memcpy(&cred, CMSG_DATA(h), sizeof cred);
    return cred.uid == 0 && cred.gid == 0 ? cred.pid : -1;
}

static const char *pass_creds(void) {
    int sv[2], on = 1, got = 0;
    socklen_t len = sizeof got;
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) != 0 || setsockopt(sv[0], SOL_SOCKET, SO_PASSCRED, &on, sizeof on) != 0 ||
        getsockopt(sv[0], SOL_SOCKET, SO_PASSCRED, &got, &len) != 0 || got != 1) {
        return "SCM_CREDENTIALS: SO_PASSCRED";
    }
    /* The peer's pidfd is not there, as on a kernel without it: an answer of
     * 0 would be taken for a pidfd (zbus then closes fd 0, its stdin). */
    if (getsockopt(sv[0], SOL_SOCKET, 77 /* SO_PEERPIDFD */, &got, &len) != -1 || errno != ENOPROTOOPT) {
        return "SO_PEERPIDFD: ENOPROTOOPT";
    }
    pid_t pid = fork();
    if (pid == 0) {
        _exit(write(sv[1], "k", 1) != 1);
    }
    int status = 0;
    if (pid < 0 || waitpid(pid, &status, 0) != pid || !WIFEXITED(status) || WEXITSTATUS(status) != 0 ||
        write(sv[1], "p", 1) != 1) {
        return "SCM_CREDENTIALS: a child writes";
    }
    if (recv_cred(sv[0]) != pid) {
        return "SCM_CREDENTIALS: the child's pid";
    }
    if (recv_cred(sv[0]) != getpid()) {
        return "SCM_CREDENTIALS: the next read, the parent's";
    }
    close(sv[0]);
    close(sv[1]);
    return NULL;
}

/* Fds over a unix socket (SCM_RIGHTS): each comes with the message it was
 * sent with; a pipe end from a child that has exited still reads what the
 * child wrote; a socket received works; fds a too small control buffer
 * has no room for are closed (MSG_CTRUNC), and so are fds never received.
 * NULL, or what failed. */
static const char *pass_fds(void) {
    int sv[2], p[2], q[2], fds[4], n, flags;
    char buf[8];
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) != 0) {
        return "SCM_RIGHTS: socketpair";
    }
    pid_t pid = fork();
    if (pid == 0) {
        int kp[2];
        close(sv[0]);
        _exit(pipe(kp) != 0 || write(kp[1], "kid", 3) != 3 || send_fds(sv[1], 'A', &kp[0], 1) != 0);
    }
    int status = 0;
    if (pid < 0 || waitpid(pid, &status, 0) != pid || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        return "SCM_RIGHTS: a child sends a pipe's read end";
    }
    if (recv_fds(sv[0], buf, sizeof buf, fds, 4, &n, &flags) != 1 || buf[0] != 'A' || n != 1 || flags != 0
        || read(fds[0], buf, 3) != 3 || memcmp(buf, "kid", 3) != 0) {
        return "SCM_RIGHTS: the pipe end, received after the child exited, reads";
    }
    close(fds[0]);

    /* Two messages with fds: each read takes its own. */
    if (pipe(p) != 0 || send_fds(sv[1], 'B', &p[0], 1) != 0 || send_fds(sv[1], 'C', &p[1], 1) != 0) {
        return "SCM_RIGHTS: send two messages";
    }
    if (recv_fds(sv[0], buf, sizeof buf, fds, 4, &n, &flags) != 1 || buf[0] != 'B' || n != 1) {
        return "SCM_RIGHTS: the first read stops before the second message's fds";
    }
    int r = fds[0];
    if (recv_fds(sv[0], buf, sizeof buf, fds, 4, &n, &flags) != 1 || buf[0] != 'C' || n != 1) {
        return "SCM_RIGHTS: the second read takes the second message's fds";
    }
    if (write(fds[0], "w", 1) != 1 || read(r, buf, 1) != 1 || buf[0] != 'w') {
        return "SCM_RIGHTS: both ends of a pipe received";
    }
    close(r);
    close(fds[0]);

    /* A socket received works. */
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, q) != 0 || send_fds(sv[1], 'D', &q[1], 1) != 0) {
        return "SCM_RIGHTS: send a socket";
    }
    close(q[1]);
    if (recv_fds(sv[0], buf, sizeof buf, fds, 4, &n, &flags) != 1 || n != 1 || write(fds[0], "s", 1) != 1
        || read(q[0], buf, 1) != 1 || buf[0] != 's') {
        return "SCM_RIGHTS: a socket received";
    }
    close(fds[0]);
    close(q[0]);

    /* Room for one of two: the other is closed. */
    int two[2] = {p[0], p[1]};
    if (send_fds(sv[1], 'E', two, 2) != 0 || recv_fds(sv[0], buf, sizeof buf, fds, 1, &n, &flags) != 1 || n != 1
        || !(flags & MSG_CTRUNC)) {
        return "SCM_RIGHTS: MSG_CTRUNC";
    }
    close(fds[0]);
    close(p[0]);
    close(p[1]);

    /* Never received: closed. The pipe's last write end goes with the
     * conversation, and its read end sees EOF. */
    if (pipe(p) != 0) {
        return "SCM_RIGHTS: pipe";
    }
    if (send_fds(sv[1], 'G', &p[1], 1) != 0) {
        return "SCM_RIGHTS: send a write end";
    }
    close(p[1]);
    close(sv[0]);
    close(sv[1]);
    if (read(p[0], buf, 1) != 0) {
        return "SCM_RIGHTS: fds never received are closed";
    }
    close(p[0]);
    return NULL;
}

/* Record sockets (SOCK_SEQPACKET): each write a record read whole, writev's
 * pieces one record, a record cut short says MSG_TRUNC, one over 8 KiB is
 * EMSGSIZE, a record socket received in SCM_RIGHTS is
 * still one, and a stream client of a record listener is refused. NULL, or
 * what failed. */
static const char *seqpacket(void) {
    static char big[9000];
    char buf[64];
    int sv[2], q[2], fds[4], n, flags, type = 0;
    socklen_t len = sizeof type;
    if (socketpair(AF_UNIX, SOCK_SEQPACKET, 0, sv) != 0
        || getsockopt(sv[0], SOL_SOCKET, SO_TYPE, &type, &len) != 0 || type != SOCK_SEQPACKET) {
        return "SOCK_SEQPACKET: socketpair, SO_TYPE";
    }
    if (write(sv[0], "one", 3) != 3 || write(sv[0], "three", 5) != 5 || read(sv[1], buf, sizeof buf) != 3
        || read(sv[1], buf, sizeof buf) != 5 || memcmp(buf, "three", 5) != 0) {
        return "SOCK_SEQPACKET: two writes are two records";
    }
    struct iovec iov[2] = {{"ab", 2}, {"cd", 2}};
    if (writev(sv[0], iov, 2) != 4 || read(sv[1], buf, sizeof buf) != 4 || memcmp(buf, "abcd", 4) != 0) {
        return "SOCK_SEQPACKET: writev's pieces are one record";
    }
    if (write(sv[0], "0123456789", 10) != 10 || write(sv[0], "next", 4) != 4
        || recv_fds(sv[1], buf, 4, fds, 0, &n, &flags) != 4 || !(flags & MSG_TRUNC)
        || read(sv[1], buf, sizeof buf) != 4 || memcmp(buf, "next", 4) != 0) {
        return "SOCK_SEQPACKET: MSG_TRUNC, then the next record";
    }
    errno = 0;
    if (write(sv[0], big, sizeof big) != -1 || errno != EMSGSIZE) {
        return "SOCK_SEQPACKET: EMSGSIZE";
    }
    if (socketpair(AF_UNIX, SOCK_SEQPACKET, 0, q) != 0 || send_fds(sv[0], 'R', &q[1], 1) != 0) {
        return "SOCK_SEQPACKET: send a record socket";
    }
    close(q[1]);
    if (recv_fds(sv[1], buf, sizeof buf, fds, 4, &n, &flags) != 1 || n != 1 || writev(fds[0], iov, 2) != 4
        || read(q[0], buf, sizeof buf) != 4) {
        return "SOCK_SEQPACKET: a record socket received keeps records";
    }
    close(fds[0]);
    close(q[0]);
    close(sv[0]);
    close(sv[1]);
    struct sockaddr_un a;
    socklen_t alen = un_addr(&a, "/tmp/.linux-smoke-seq");
    int ls = socket(AF_UNIX, SOCK_SEQPACKET, 0);
    if (ls < 0 || bind(ls, (struct sockaddr *)&a, alen) != 0 || listen(ls, 4) != 0) {
        return "SOCK_SEQPACKET: a record listener";
    }
    int c = socket(AF_UNIX, SOCK_STREAM, 0);
    if (c < 0 || connect(c, (struct sockaddr *)&a, alen) == 0) {
        return "SOCK_SEQPACKET: a stream client of a record listener is refused";
    }
    close(c);
    close(ls);
    return NULL;
}

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
    const char *e = pass_fds();
    check(e == NULL, e != NULL ? e : "SCM_RIGHTS");
    e = memfd_seals();
    check(e == NULL, e != NULL ? e : "memfd seals");
    e = pass_creds();
    check(e == NULL, e != NULL ? e : "SCM_CREDENTIALS");
    e = seqpacket();
    check(e == NULL, e != NULL ? e : "SOCK_SEQPACKET");
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
    /* Only the member's own object counts (issue #456): another pipe's
     * traffic is no edge, a new byte is one, drained or not. */
    int r[2] = {-1, -1};
    check(pipe(r) == 0 && write(q[1], &c, 1) == 1 && epoll_wait(ep, ev, 4, 0) == 1 && ev[0].data.u64 == 30 &&
              write(r[1], &c, 1) == 1 && read(r[0], &c, 1) == 1 && epoll_wait(ep, ev, 4, 0) == 0,
          "EPOLLET: another pipe's traffic is no edge");
    check(write(q[1], &c, 1) == 1 && epoll_wait(ep, ev, 4, 0) == 1 && ev[0].data.u64 == 30 && epoll_wait(ep, ev, 4, 0) == 0 &&
              read(q[0], &c, 1) == 1 && read(q[0], &c, 1) == 1 && epoll_wait(ep, ev, 4, 0) == 0,
          "EPOLLET: new data is a new edge, once");
    close(r[0]);
    close(r[1]);
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

/* The clocks (issue #317): CLOCK_MONOTONIC and CLOCK_BOOTTIME count from
 * boot, not from 1970, which also puts their absolute deadlines there
 * (clock_nanosleep, timerfd and a FUTEX_WAIT_BITSET wait, through
 * pthread_cond_timedwait on a monotonic condition); a wall-clock deadline
 * still works; the resolutions; an id no clock has is EINVAL. */
static long long ns_of(const struct timespec *ts) {
    return (long long)ts->tv_sec * 1000000000LL + ts->tv_nsec;
}

static void check_clocks(void) {
    struct timespec wall, mono, boot, res, t0, t1, at;
    check(clock_gettime(CLOCK_REALTIME, &wall) == 0 && clock_gettime(CLOCK_MONOTONIC, &mono) == 0 &&
              clock_gettime(CLOCK_BOOTTIME, &boot) == 0 && mono.tv_sec < wall.tv_sec / 2 &&
              ns_of(&boot) >= ns_of(&mono) && ns_of(&boot) - ns_of(&mono) < 60000000000LL,
          "CLOCK_MONOTONIC and CLOCK_BOOTTIME count from boot");
    check(clock_getres(CLOCK_MONOTONIC, &res) == 0 && res.tv_sec == 0 && res.tv_nsec == 1 &&
              clock_getres(CLOCK_REALTIME, &res) == 0 && res.tv_nsec == 1000,
          "clock_getres");
    errno = 0;
    check(clock_gettime((clockid_t)1234, &res) == -1 && errno == EINVAL, "clock_gettime: an unknown clock is EINVAL");

    /* 50 ms ahead on each clock, absolute: none returns before then. */
    clockid_t clocks[2] = {CLOCK_MONOTONIC, CLOCK_REALTIME};
    for (int i = 0; i < 2; i++) {
        clock_gettime(CLOCK_MONOTONIC, &t0);
        clock_gettime(clocks[i], &at);
        at.tv_nsec += 50000000;
        if (at.tv_nsec >= 1000000000) {
            at.tv_sec++;
            at.tv_nsec -= 1000000000;
        }
        check(clock_nanosleep(clocks[i], TIMER_ABSTIME, &at, NULL) == 0, "clock_nanosleep: TIMER_ABSTIME");
        clock_gettime(CLOCK_MONOTONIC, &t1);
        check(ns_of(&t1) - ns_of(&t0) >= 40000000LL && ns_of(&t1) - ns_of(&t0) < 5000000000LL,
              i ? "clock_nanosleep: 50 ms ahead on CLOCK_REALTIME" : "clock_nanosleep: 50 ms ahead on CLOCK_MONOTONIC");

        struct itimerspec its = {{0, 0}, {0, 0}};
        uint64_t v;
        int fd = timerfd_create(clocks[i], 0);
        clock_gettime(CLOCK_MONOTONIC, &t0);
        clock_gettime(clocks[i], &its.it_value);
        its.it_value.tv_nsec += 50000000;
        if (its.it_value.tv_nsec >= 1000000000) {
            its.it_value.tv_sec++;
            its.it_value.tv_nsec -= 1000000000;
        }
        check(fd >= 0 && timerfd_settime(fd, TFD_TIMER_ABSTIME, &its, NULL) == 0 && read(fd, &v, 8) == 8 && v == 1,
              "timerfd: TFD_TIMER_ABSTIME");
        clock_gettime(CLOCK_MONOTONIC, &t1);
        check(ns_of(&t1) - ns_of(&t0) >= 40000000LL && ns_of(&t1) - ns_of(&t0) < 5000000000LL,
              i ? "timerfd: 50 ms ahead on CLOCK_REALTIME" : "timerfd: 50 ms ahead on CLOCK_MONOTONIC");
        close(fd);
    }

    pthread_condattr_t ca;
    pthread_cond_t cv;
    pthread_mutex_t mu = PTHREAD_MUTEX_INITIALIZER;
    check(pthread_condattr_init(&ca) == 0 && pthread_condattr_setclock(&ca, CLOCK_MONOTONIC) == 0 &&
              pthread_cond_init(&cv, &ca) == 0,
          "pthread_condattr_setclock(CLOCK_MONOTONIC)");
    clock_gettime(CLOCK_MONOTONIC, &t0);
    at = t0;
    at.tv_nsec += 50000000;
    if (at.tv_nsec >= 1000000000) {
        at.tv_sec++;
        at.tv_nsec -= 1000000000;
    }
    pthread_mutex_lock(&mu);
    int r = pthread_cond_timedwait(&cv, &mu, &at);
    pthread_mutex_unlock(&mu);
    clock_gettime(CLOCK_MONOTONIC, &t1);
    check(r == ETIMEDOUT && ns_of(&t1) - ns_of(&t0) >= 40000000LL && ns_of(&t1) - ns_of(&t0) < 5000000000LL,
          "pthread_cond_timedwait: 50 ms ahead on CLOCK_MONOTONIC");
    pthread_cond_destroy(&cv);
}

/* The parent-death signal (issue #491): a process whose parent is killed
 * gets the signal it asked for with PR_SET_PDEATHSIG, as bubblewrap's
 * helpers do (gdk-pixbuf's loader outlived WebKitGTK's crashed UI
 * process); a forked child starts without one. A middle process forks
 * the one that asks, and is killed once that one is ready; the asker
 * holds a pipe's write end, whose EOF says it ended. */
static void check_pdeathsig(void) {
    int ready[2], gone[2];
    if (pipe(ready) != 0 || pipe(gone) != 0) {
        check(0, "pipe");
        return;
    }
    pid_t mid = fork();
    if (mid == 0) {
        pid_t c = fork();
        if (c == 0) {
            int sig = -1;
            if (prctl(PR_SET_PDEATHSIG, SIGTERM) != 0 || prctl(PR_GET_PDEATHSIG, &sig) != 0 || sig != SIGTERM) {
                _exit(1);
            }
            pid_t inner = fork();
            if (inner == 0) {
                sig = -1;
                _exit(prctl(PR_GET_PDEATHSIG, &sig) == 0 && sig == 0 ? 0 : 1);
            }
            int st;
            if (inner < 0 || waitpid(inner, &st, 0) != inner || !WIFEXITED(st) || WEXITSTATUS(st) != 0) {
                _exit(1);
            }
            pid_t me = getpid();
            close(ready[0]);
            close(gone[0]);
            write(ready[1], &me, sizeof me);
            for (;;) {
                pause();
            }
        }
        close(ready[0]);
        close(ready[1]);
        close(gone[0]);
        close(gone[1]);
        for (;;) {
            pause();
        }
    }
    close(ready[1]);
    close(gone[1]);
    pid_t asker = 0;
    int got = mid > 0 && read(ready[0], &asker, sizeof asker) == sizeof asker;
    check(got, "pdeathsig: the child set it, and its own fork has none");
    if (mid > 0) {
        kill(mid, SIGKILL);
        waitpid(mid, NULL, 0);
    }
    struct pollfd p = { gone[0], POLLIN, 0 };
    char c;
    int ended = got && poll(&p, 1, 5000) == 1 && read(gone[0], &c, 1) == 0;
    check(ended, "pdeathsig: sent when the parent is killed");
    if (got && !ended) {
        kill(asker, SIGKILL);
    }
    close(ready[0]);
    close(gone[0]);
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
    /* No extended attributes, as on a Linux file system without them:
     * what shutil.copy2 (ensurepip, pipx) copies and skips. */
    char xa[16];
    check(listxattr("/tmp", xa, sizeof xa) == 0 && flistxattr(fd, xa, sizeof xa) == 0,
          "listxattr: no attributes");
    check(getxattr("/tmp", "user.x", xa, sizeof xa) == -1 && errno == ENODATA, "getxattr: ENODATA");
    check(setxattr("/tmp", "user.x", "1", 1, 0) == -1 && errno == EOPNOTSUPP, "setxattr: EOPNOTSUPP");
    check(listxattr("/no/such", xa, sizeof xa) == -1 && errno == ENOENT, "listxattr of nothing: ENOENT");
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
    /* What bubblewrap and glycin (WebKitGTK's image loaders) set up a
     * sandbox with: no namespaces (glycin then runs its loaders as they
     * are), no capabilities, the parent-death signal and no_new_privs
     * accepted, the overflow ids. */
    int pdeath = -1;
    check(prctl(PR_SET_PDEATHSIG, SIGKILL) == 0 && prctl(PR_GET_PDEATHSIG, &pdeath) == 0 && pdeath == SIGKILL &&
              prctl(PR_SET_PDEATHSIG, 0) == 0 && prctl(PR_GET_PDEATHSIG, &pdeath) == 0 && pdeath == 0 &&
              prctl(PR_SET_PDEATHSIG, 99) == -1 && errno == EINVAL &&
              prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) == 0 && prctl(PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) == 1,
          "prctl: pdeathsig, no_new_privs");
    check(unshare(0) == 0 && unshare(CLONE_NEWUSER) == -1 && errno == EINVAL &&
              syscall(SYS_clone, CLONE_NEWPID | SIGCHLD, 0, 0, 0, 0) == -1 && errno == EINVAL,
          "unshare, clone: no namespaces");
    struct { uint32_t version; int pid; } caphdr = { 0x20080522, 0 };
    uint32_t caps[6] = { 1, 1, 1, 1, 1, 1 };
    check(syscall(SYS_capget, &caphdr, caps) == 0 && !(caps[0] | caps[1] | caps[2] | caps[3] | caps[4] | caps[5]) &&
              syscall(SYS_capset, &caphdr, caps) == 0,
          "capget: none");
    caphdr.version = 1;
    check(syscall(SYS_capget, &caphdr, NULL) == -1 && errno == EINVAL && caphdr.version == 0x20080522,
          "capget: an unknown version is told the current one");
    char ov[16] = "";
    fd = open("/proc/sys/kernel/overflowuid", O_RDONLY);
    check(fd >= 0 && read(fd, ov, sizeof ov - 1) == 6 && strcmp(ov, "65534\n") == 0, "overflowuid");
    close(fd);
    char *m = mmap(NULL, 3 * 4096, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    check(m != MAP_FAILED && mremap(m, 3 * 4096, 4096, 0) == m && mremap(m, 4096, 8192, MREMAP_MAYMOVE) == MAP_FAILED &&
              errno == ENOMEM,
          "mremap: shrink in place, no growth");
    if (m != MAP_FAILED) {
        munmap(m, 4096);
        check(mremap(m, 4096, 8192, 0) == MAP_FAILED && errno == EFAULT, "mremap of a hole: EFAULT");
    }
    /* A reservation bigger than the memory (WebKit's JavaScript engine
     * takes 4 GiB aligned to 4 GiB): address space only, its pages
     * mapped as they are touched; unmapped again at once. */
    const size_t big = (size_t)5 << 30;
    char *r5 = mmap(NULL, big, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_NORESERVE, -1, 0);
    check(r5 != MAP_FAILED, "mmap: a 5 GiB reservation");
    if (r5 != MAP_FAILED) {
        char *last = r5 + big - 4096;
        check(mprotect(r5, 4096, PROT_READ | PROT_WRITE) == 0 && mprotect(last, 4096, PROT_READ | PROT_WRITE) == 0 &&
                  (r5[0] = 1, last[0] = 2, r5[0] + last[0] == 3),
              "mmap: both ends of the reservation usable");
        check(munmap(r5, big) == 0, "munmap: the reservation");
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
    // CPU time: the process's grows while it computes.
    struct timespec c0, c1;
    struct rusage ru;
    volatile unsigned long spin = 0;
    check(clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &c0) == 0, "clock_gettime(CLOCK_PROCESS_CPUTIME_ID)");
    for (unsigned long i = 0; i < 20000000; i++) {
        spin += i;
    }
    check(clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &c1) == 0 &&
              (c1.tv_sec > c0.tv_sec || (c1.tv_sec == c0.tv_sec && c1.tv_nsec > c0.tv_nsec)),
          "CPU time grows while computing");
    check(getrusage(RUSAGE_SELF, &ru) == 0 && (ru.ru_utime.tv_sec > 0 || ru.ru_utime.tv_usec > 0) &&
              getrusage(RUSAGE_CHILDREN, &ru) == 0 && getrusage(42, &ru) == -1 && errno == EINVAL,
          "getrusage");
    int ffd = open("/tmp/linux-smoke.fadvise", O_RDWR | O_CREAT | O_TRUNC, 0644);
    check(ffd >= 0 && posix_fadvise(ffd, 0, 0, POSIX_FADV_SEQUENTIAL) == 0 && posix_fadvise(ffd, 0, 0, 99) == EINVAL,
          "posix_fadvise: a hint");
    close(ffd);
    unlink("/tmp/linux-smoke.fadvise");
    long mb = syscall(SYS_membarrier, 0, 0);
    check(mb > 0 && (mb & 8) && syscall(SYS_membarrier, 16, 0) == 0 && syscall(SYS_membarrier, 8, 0) == 0,
          "membarrier: query, register, private expedited");
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
    /* A trailing slash names a directory: a file with one is ENOTDIR, a
     * symlink to a directory is followed through it (issue #583). */
    check(stat("/tmp/linux-smoke.trunc/", &st) < 0 && errno == ENOTDIR, "stat of FILE/: ENOTDIR");
    check(open("/tmp/linux-smoke.trunc/", O_RDONLY) < 0 && errno == ENOTDIR, "open of FILE/: ENOTDIR");
    check(symlink("/tmp", "/tmp/linux-smoke.dirlink") == 0 && stat("/tmp/linux-smoke.dirlink/", &st) == 0 &&
              S_ISDIR(st.st_mode) && lstat("/tmp/linux-smoke.dirlink/", &st) == 0 && S_ISDIR(st.st_mode),
          "LINK-TO-DIR/ follows the link");
    unlink("/tmp/linux-smoke.dirlink");
    unlink("/tmp/linux-smoke.link");
    unlink("/tmp/linux-smoke.trunc");
    /* A dangling link's own owner and mode can be set (apk sets a .so.N
     * link's before it unpacks the library), not what it points to. */
    check(symlink("linux-smoke.nothing", "/tmp/linux-smoke.dangle") == 0 && lchown("/tmp/linux-smoke.dangle", 0, 0) == 0 &&
              fchownat(AT_FDCWD, "/tmp/linux-smoke.dangle", 0, 0, AT_SYMLINK_NOFOLLOW) == 0 &&
              chown("/tmp/linux-smoke.dangle", 0, 0) < 0 && errno == ENOENT,
          "lchown of a dangling link");
    unlink("/tmp/linux-smoke.dangle");

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

/* One raw query for github.com's A record, sent as the resolver sends
 * it (`__res_msend`), with what came back: run when `dns` failed, to say
 * which step loses the answer. Each probe has its own socket: a query to
 * `ns` alone; to `ns` then to `dead`; to `dead` then to `ns`, and `ns`
 * again after a wait. (No printf: musl's wants the quad-float builtins the
 * static link has not.) */
static void out_num(long n) {
    char b[24];
    int i = sizeof b;
    unsigned long u = n < 0 ? -(unsigned long)n : (unsigned long)n;
    b[--i] = 0;
    do {
        b[--i] = '0' + u % 10;
        u /= 10;
    } while (u);
    if (n < 0) {
        b[--i] = '-';
    }
    out(b + i);
}

static void probe_wait(int fd, const char *what) {
    struct pollfd p = { .fd = fd, .events = POLLIN };
    struct sockaddr_in from = { 0 };
    socklen_t fl = sizeof from;
    unsigned char a[512];
    int ready = poll(&p, 1, 3000);
    ssize_t n = ready > 0 ? recvfrom(fd, a, sizeof a, 0, (struct sockaddr *)&from, &fl) : -1;
    out("probe: ");
    out(what);
    out(": poll ");
    out_num(ready);
    out(", recvfrom ");
    out_num(n);
    if (n < 0) {
        out(" ");
        out(strerror(errno));
    }
    out(" from ");
    // (inet_ntoa is a printf too.)
    for (int i = 0; i < 4; i++) {
        out_num(((unsigned char *)&from.sin_addr)[i]);
        out(i < 3 ? "." : ":");
    }
    out_num(ntohs(from.sin_port));
    out(" rcode ");
    out_num(n >= 4 ? a[3] & 15 : -1);
    out("\n");
}

static void probe_send(int fd, const unsigned char *q, size_t len, const struct sockaddr_in *to, const char *what) {
    ssize_t n = sendto(fd, q, len, 0, (const struct sockaddr *)to, sizeof *to);
    out("probe: sendto ");
    out(what);
    out(" ");
    out_num(n);
    if (n < 0) {
        out(" ");
        out(strerror(errno));
    }
    out("\n");
}

/* `inet6`: IPv6 datagrams through the layer, between two of its own
 * sockets: an AF_INET6 one's port is on `::` (getsockname names it), a
 * datagram to [::1] reaches it from ::1, and one from an AF_INET socket
 * to 127.0.0.1 from ::ffff:127.0.0.1 (dual stack, as on Linux). */
static int inet6_udp(void) {
    static const unsigned char mapped[16] = { [10] = 0xff, [11] = 0xff, [12] = 127, [15] = 1 };
    struct sockaddr_in6 me = { 0 }, to = { 0 }, from;
    struct sockaddr_in to4 = { 0 };
    socklen_t len = sizeof me;
    char buf[8];
    int s = socket(AF_INET6, SOCK_DGRAM, 0);
    int c6 = socket(AF_INET6, SOCK_DGRAM, 0);
    int c4 = socket(AF_INET, SOCK_DGRAM, 0);
    check(s >= 0 && c6 >= 0 && c4 >= 0, "socket AF_INET6/AF_INET SOCK_DGRAM");
    check(getsockname(s, (struct sockaddr *)&me, &len) == 0 && me.sin6_family == AF_INET6
              && me.sin6_port != 0 && IN6_IS_ADDR_UNSPECIFIED(&me.sin6_addr),
          "getsockname: [::]:port");
    to.sin6_family = AF_INET6;
    to.sin6_port = me.sin6_port;
    to.sin6_addr = in6addr_loopback;
    check(sendto(c6, "six", 3, 0, (struct sockaddr *)&to, sizeof to) == 3, "sendto [::1]");
    len = sizeof from;
    check(recvfrom(s, buf, sizeof buf, 0, (struct sockaddr *)&from, &len) == 3 && memcmp(buf, "six", 3) == 0
              && from.sin6_family == AF_INET6 && IN6_IS_ADDR_LOOPBACK(&from.sin6_addr),
          "recvfrom: from ::1");
    to4.sin_family = AF_INET;
    to4.sin_port = me.sin6_port;
    to4.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    check(sendto(c4, "four", 4, 0, (struct sockaddr *)&to4, sizeof to4) == 4, "sendto 127.0.0.1");
    len = sizeof from;
    check(recvfrom(s, buf, sizeof buf, 0, (struct sockaddr *)&from, &len) == 4 && memcmp(buf, "four", 4) == 0
              && memcmp(from.sin6_addr.s6_addr, mapped, 16) == 0,
          "recvfrom: from ::ffff:127.0.0.1");
    close(s);
    close(c6);
    close(c4);
    out(failures ? "LINUX-INET6 FAIL\n" : "LINUX-INET6 OK\n");
    return failures != 0;
}

static void dns_probe(const char *ns, const char *dead) {
    static const unsigned char q[] = { 0x12, 0x34, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 6, 'g', 'i', 't', 'h', 'u',
                                       'b', 3, 'c', 'o', 'm', 0, 0, 1, 0, 1 };
    struct sockaddr_in to_ns = { .sin_family = AF_INET, .sin_port = htons(53) }, to_dead = to_ns;
    inet_pton(AF_INET, ns, &to_ns.sin_addr);
    inet_pton(AF_INET, dead, &to_dead.sin_addr);
    for (int step = 0; step < 3; step++) {
        int fd = socket(AF_INET, SOCK_DGRAM | SOCK_NONBLOCK, 0);
        if (step == 2) {
            probe_send(fd, q, sizeof q, &to_dead, "dead");
            probe_send(fd, q, sizeof q, &to_ns, "ns");
        } else {
            probe_send(fd, q, sizeof q, &to_ns, "ns");
            if (step == 1) {
                probe_send(fd, q, sizeof q, &to_dead, "dead");
            }
        }
        probe_wait(fd, step == 0 ? "ns alone" : step == 1 ? "ns then dead" : "dead then ns");
        if (step == 2) {
            probe_send(fd, q, sizeof q, &to_ns, "ns again");
            probe_wait(fd, "ns again");
        }
        close(fd);
    }
}

/* The answer to a query sent to `ns` is read after a query to `dead` went
 * from the same socket: it is still there, and from `ns` (a layer that
 * connected the socket to the last address sent to reported that as the
 * sender, and dropped what the earlier one had answered). */
static void dns_sender(const char *ns, const char *dead) {
    static const unsigned char q[] = { 0x56, 0x78, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0, 6, 'g', 'i', 't', 'h', 'u',
                                       'b', 3, 'c', 'o', 'm', 0, 0, 1, 0, 1 };
    struct sockaddr_in to_ns = { .sin_family = AF_INET, .sin_port = htons(53) }, to_dead = to_ns, from = { 0 };
    struct pollfd p;
    socklen_t fl = sizeof from;
    unsigned char a[512];
    int fd = socket(AF_INET, SOCK_DGRAM | SOCK_NONBLOCK, 0);
    inet_pton(AF_INET, ns, &to_ns.sin_addr);
    inet_pton(AF_INET, dead, &to_dead.sin_addr);
    p.fd = fd;
    p.events = POLLIN;
    int sent = sendto(fd, q, sizeof q, 0, (struct sockaddr *)&to_ns, sizeof to_ns) == (ssize_t)sizeof q;
    int ready = sent && poll(&p, 1, 10000) == 1;
    int later = ready && sendto(fd, q, sizeof q, 0, (struct sockaddr *)&to_dead, sizeof to_dead) == (ssize_t)sizeof q;
    ssize_t n = later ? recvfrom(fd, a, sizeof a, 0, (struct sockaddr *)&from, &fl) : -1;
    check(n >= 12 && a[0] == 0x56 && a[1] == 0x78 && from.sin_addr.s_addr == to_ns.sin_addr.s_addr &&
              from.sin_port == htons(53),
          "a datagram's sender is its own, after a send elsewhere");
    close(fd);
}

/* CPU faults raise their signals (user/c/fault_smoke.c natively): SIGILL
 * for an undefined instruction and SIGSEGV for a PROT_NONE page, with
 * si_code and si_addr, left with siglongjmp; then a SIGSEGV handler that
 * makes the page writable and returns, after which the store has run with
 * the registers it had (the ones a syscall's return does not keep: x86's
 * rcx, r11 and flags, aarch64's NZCV, the result and number registers). */
static sigjmp_buf fault_env;
static volatile int fault_sig, fault_code;
static void *volatile fault_addr;
static char *fault_page;

static void on_fault_leave(int sig, siginfo_t *si, void *uc) {
    (void)uc;
    fault_sig = sig;
    fault_code = si->si_code;
    fault_addr = si->si_addr;
    siglongjmp(fault_env, 1);
}

static void on_fault_fix(int sig, siginfo_t *si, void *uc) {
    (void)uc;
    fault_sig = sig;
    fault_addr = si->si_addr;
    mprotect(fault_page, 4096, PROT_READ | PROT_WRITE);
}

static void fault_catch(int sig, void (*fn)(int, siginfo_t *, void *)) {
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_sigaction = fn;
    sa.sa_flags = SA_SIGINFO;
    sigaction(sig, &sa, NULL);
}

static int fault_store_resumes(volatile char *p) {
#if defined(__x86_64__)
    register unsigned long rax __asm__("rax") = 0x1111111111111111UL;
    register unsigned long rcx __asm__("rcx") = 0x2222222222222222UL;
    register unsigned long r11 __asm__("r11") = 0x3333333333333333UL;
    unsigned char cf;
    __asm__ volatile("stc\n\tmovb $1, (%[p])\n\tsetc %[cf]"
                     : "+r"(rax), "+r"(rcx), "+r"(r11), [cf] "=r"(cf)
                     : [p] "r"(p)
                     : "memory");
    return rax == 0x1111111111111111UL && rcx == 0x2222222222222222UL && r11 == 0x3333333333333333UL && cf == 1 &&
           p[0] == 1;
#elif defined(__aarch64__)
    register unsigned long x0 __asm__("x0") = 0x1111111111111111UL;
    register unsigned long x8 __asm__("x8") = 0x2222222222222222UL;
    unsigned long nzcv;
    __asm__ volatile("cmp xzr, xzr\n\tstrb %w[one], [%[p]]\n\tmrs %[f], nzcv"
                     : "+r"(x0), "+r"(x8), [f] "=r"(nzcv)
                     : [p] "r"(p), [one] "r"(1)
                     : "memory", "cc");
    return x0 == 0x1111111111111111UL && x8 == 0x2222222222222222UL && nzcv >> 28 == 0x6 && p[0] == 1;
#else
    register unsigned long a0 __asm__("a0") = 0x1111111111111111UL;
    register unsigned long a7 __asm__("a7") = 0x2222222222222222UL;
    __asm__ volatile("sb %[one], 0(%[p])" : "+r"(a0), "+r"(a7) : [p] "r"(p), [one] "r"(1) : "memory");
    return a0 == 0x1111111111111111UL && a7 == 0x2222222222222222UL && p[0] == 1;
#endif
}

#if defined(__x86_64__)
/* x86's single step: three SIGTRAPs, one per instruction after popfq; the
 * handler starts with TF clear (else it would trap again with SIGTRAP
 * blocked), rt_sigreturn puts it back, and the third handler turns it off
 * in the ucontext. */
static volatile int steps;
static void on_step(int sig, siginfo_t *si, void *ucv) {
    ucontext_t *uc = ucv;
    unsigned long flags;
    (void)sig;
    __asm__ volatile("pushfq\n\tpopq %0" : "=r"(flags));
    if ((flags & 0x100) != 0 || si->si_code != TRAP_TRACE) {
        steps = -100;
    }
    if (++steps == 3) {
        uc->uc_mcontext.gregs[REG_EFL] &= ~0x100L;
    }
}

static void check_single_step(void) {
    fault_catch(SIGTRAP, on_step);
    steps = 0;
    __asm__ volatile("pushfq\n\torq $0x100, (%%rsp)\n\tpopfq\n\tnop\n\tnop\n\tnop\n\tnop\n\tnop" ::: "memory", "cc");
    signal(SIGTRAP, SIG_DFL);
    check(steps == 3, "single step: a SIGTRAP per instruction, the handler without TF");
}
#endif

static void check_faults(void) {
    fault_catch(SIGILL, on_fault_leave);
    fault_sig = 0;
    if (sigsetjmp(fault_env, 1) == 0) {
#if defined(__x86_64__)
        __asm__ volatile("ud2");
#elif defined(__aarch64__)
        __asm__ volatile("udf #0");
#else
        __asm__ volatile("unimp");
#endif
    }
    signal(SIGILL, SIG_DFL);
#if defined(__x86_64__)
    int ill = ILL_ILLOPN;
#else
    int ill = ILL_ILLOPC;
#endif
    check(fault_sig == SIGILL && fault_code == ill && fault_addr != NULL, "SIGILL from an undefined instruction");

    fault_page = mmap(NULL, 4096, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    check(fault_page != MAP_FAILED, "mmap PROT_NONE");
    if (fault_page == MAP_FAILED) {
        return;
    }
    fault_catch(SIGSEGV, on_fault_leave);
    fault_sig = 0;
    if (sigsetjmp(fault_env, 1) == 0) {
        (void)*(volatile char *)(fault_page + 8);
    }
    sigset_t now;
    sigprocmask(SIG_BLOCK, NULL, &now);
    check(fault_sig == SIGSEGV && fault_code == SEGV_ACCERR && fault_addr == fault_page + 8 &&
              !sigismember(&now, SIGSEGV),
          "SIGSEGV from a PROT_NONE page, si_addr, mask back after siglongjmp");

    fault_catch(SIGSEGV, on_fault_fix);
    fault_sig = 0;
    int kept = fault_store_resumes(fault_page + 16);
    signal(SIGSEGV, SIG_DFL);
    check(fault_sig == SIGSEGV && fault_addr == fault_page + 16 && kept,
          "a SIGSEGV handler returns to the store, registers kept");
    munmap(fault_page, 4096);
#if defined(__x86_64__)
    check_single_step();
#endif
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
    if (argc > 1 && strcmp(argv[1], "inet6") == 0) {
        return inet6_udp();
    }
    if (argc > 2 && strcmp(argv[1], "unix") == 0) {
        return unix_client(argv[2]);
    }
    /* `enospc FILE`: on a full filesystem (fileio_smoke filled it) a write
     * says ENOSPC, as does a pwrite into the hole a grown file has; apk
     * used to see EIO and stop with "I/O error". */
    if (argc > 2 && strcmp(argv[1], "enospc") == 0) {
        static char mib[1 << 20];
        int fd = open(argv[2], O_RDWR | O_CREAT | O_TRUNC, 0644);
        check(fd >= 0, "open on a full filesystem");
        errno = 0;
        check(write(fd, mib, sizeof mib) == -1 && errno == ENOSPC, "write: ENOSPC");
        errno = 0;
        check(ftruncate(fd, 1 << 20) == 0 && pwrite(fd, mib, 4096, 4096) == -1 && errno == ENOSPC,
              "pwrite into a hole: ENOSPC");
        out(failures ? "LINUX-ENOSPC FAIL\n" : "LINUX-ENOSPC OK\n");
        return failures != 0;
    }
    /* `enosys`: a syscall no arch has gets ENOSYS (with the layer's trace
     * on, user/tests/kernel.sh, it is also reported on the console). */
    if (argc > 1 && strcmp(argv[1], "enosys") == 0) {
        errno = 0;
        int ok = syscall(4000, 1, 2, 3) == -1 && errno == ENOSYS;
        out(ok ? "LINUX-ENOSYS OK\n" : "LINUX-ENOSYS FAIL\n");
        return !ok;
    }
    /* `cwd`: getcwd names the cwd as the root sees it, also when the root
     * lies under a bind (`linux --root /mnt/x`, /mnt being /tmp/mnt). */
    if (argc > 1 && strcmp(argv[1], "cwd") == 0) {
        char b[256];
        check(getcwd(b, sizeof b) && strcmp(b, "/") == 0, "getcwd at the root");
        mkdir("/d", 0755);
        check(chdir("/d") == 0 && getcwd(b, sizeof b) && strcmp(b, "/d") == 0, "getcwd after chdir");
        int d = open("/d", O_RDONLY | O_DIRECTORY);
        check(chdir("/") == 0 && d >= 0 && fchdir(d) == 0 && getcwd(b, sizeof b) && strcmp(b, "/d") == 0,
              "getcwd after fchdir");
        check(getcwd(b, 2) == NULL && errno == ERANGE, "getcwd: a short buffer is ERANGE");
        close(d);
        rmdir("/d");
        out(failures ? "LINUX-CWD FAIL\n" : "LINUX-CWD OK\n");
        return failures != 0;
    }
    /* `dns NS DEAD`: musl's resolver (Alpine's programs: GLib, python),
     * from a root whose /etc/resolv.conf names a dead address and the
     * system's nameserver NS (kernel.sh says why in that order). It asks
     * both at once from one socket and takes the answer from whichever:
     * the layer must report each datagram's real sender (`hdata`), not the
     * last address sent to, which `dns_sender` checks directly. */
    if (argc > 1 && strcmp(argv[1], "dns") == 0) {
        struct addrinfo hints = { .ai_family = AF_INET, .ai_socktype = SOCK_STREAM }, *res = NULL;
        int r = getaddrinfo("github.com", "80", &hints, &res);
        check(r == 0 && res != NULL, r == 0 ? "getaddrinfo" : gai_strerror(r));
        if (res != NULL) {
            freeaddrinfo(res);
        }
        if (argc > 3) {
            if (r == 0) {
                dns_sender(argv[2], argv[3]);
            } else {
                dns_probe(argv[2], argv[3]);
            }
        }
        out(failures ? "LINUX-DNS FAIL\n" : "LINUX-DNS OK\n");
        return failures != 0;
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

    /* A directory of more names than one listing buffer held, emptied
     * while it is read (what rm -rf does): every entry is seen once and
     * the directory is left empty; rmdir of it with an entry left is
     * ENOTEMPTY (issue #584). */
    const char *bigdir = "/tmp/linux-smoke.big";
    char name[128];
    int made = 0, seen = 0;
    mkdir(bigdir, 0755);
    for (int i = 0; i < 300; i++) {
        /* No printf family here: on aarch64 and riscv64 it needs the
         * 128-bit float builtins this static program is not linked with. */
        strcpy(name, bigdir);
        strcat(name, "/an-entry-whose-name-is-long-000");
        size_t at = strlen(name);
        name[at - 3] = (char)('0' + i / 100);
        name[at - 2] = (char)('0' + i / 10 % 10);
        name[at - 1] = (char)('0' + i % 10);
        int f = open(name, O_WRONLY | O_CREAT | O_TRUNC, 0644);
        if (f >= 0) {
            made++;
            close(f);
        }
    }
    check(made == 300, "300 files in a directory");
    check(rmdir(bigdir) < 0 && errno == ENOTEMPTY, "rmdir of a directory with entries: ENOTEMPTY");
    d = opendir(bigdir);
    if (d) {
        struct dirent *e;
        while ((e = readdir(d)) != NULL) {
            if (e->d_name[0] == '.') {
                continue;
            }
            strcpy(name, bigdir);
            strcat(name, "/");
            strncat(name, e->d_name, sizeof name - strlen(name) - 1);
            if (unlink(name) == 0) {
                seen++;
            }
        }
        closedir(d);
    }
    check(seen == 300, "readdir while unlinking sees every entry once");
    check(rmdir(bigdir) == 0, "rmdir of the emptied directory");

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
    check_faults();
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
    check_pdeathsig();
    check_clocks();

    if (failures) {
        out("LINUX-SMOKE FAIL\n");
        return 1;
    }
    out("LINUX-SMOKE OK\n");
    return 0;
}
