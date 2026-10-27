/* unix-smoke: boot-CI guest test for AF_UNIX stream sockets over /net/unix
 * (modules/netfs/src/unix.rs, libgloss socket.c, docs/sockets-unix.md).
 *
 * socketpair both ways and EOF on close; then a listener: a second listener
 * of the same name is refused, connecting to an unknown name too, a
 * nonblocking accept with nothing queued says EAGAIN. A forked client
 * connects, they exchange a greeting and the client sends 256 KiB, more than
 * one end buffers, so its writes wait for the reader. Closing the listener
 * frees its name. An end takes 60000 bytes before anyone reads (more than
 * the 8 KiB it once held), and 48 conversations are open at once (more
 * than the 32 there once were). Prints [ OK ] unix.
 *
 * `unix_smoke echo NAME`: listen under NAME, echo one connection's bytes
 * back until it ends, exit 0 (a native server for the Linux layer's
 * clients, user/tests/kernel.sh).
 *
 * `unix_smoke seqpacket`: record sockets (SOCK_SEQPACKET) instead; see
 * seqpacket() below. Prints [ OK ] unix seqpacket.
 */
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/uio.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>

#define NAME "/tmp/.unix-smoke"
#define BULK (256 * 1024)
/* Socketpairs a process (and its child) opens at once: two conversations
 * each, several fds each. */
#define PAIRS 12
#define AHEAD 60000

static int fail(const char *what) {
    printf("[ FAIL ] unix %s (errno %d)\n", what, errno);
    return 1;
}

/* Read exactly n bytes (0 if EOF came first, -1 on error). */
static ssize_t read_full(int fd, void *buf, size_t n) {
    size_t got = 0;
    while (got < n) {
        ssize_t r = read(fd, (char *)buf + got, n - got);
        if (r < 0) {
            return -1;
        }
        if (r == 0) {
            return 0;
        }
        got += (size_t)r;
    }
    return (ssize_t)got;
}

static int write_full(int fd, const void *buf, size_t n) {
    size_t put = 0;
    while (put < n) {
        ssize_t w = write(fd, (const char *)buf + put, n - put);
        if (w <= 0) {
            return -1;
        }
        put += (size_t)w;
    }
    return 0;
}

static struct sockaddr_un addr(const char *name) {
    struct sockaddr_un a;
    memset(&a, 0, sizeof a);
    a.sun_family = AF_UNIX;
    strncpy(a.sun_path, name, sizeof a.sun_path - 1);
    return a;
}

static int listener(const char *name) {
    struct sockaddr_un a = addr(name);
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0) {
        return -1;
    }
    if (bind(fd, (struct sockaddr *)&a, sizeof a) < 0 || listen(fd, 4) < 0) {
        int e = errno;
        close(fd);
        errno = e;
        return -1;
    }
    return fd;
}

static int client(void) {
    static char bulk[BULK];
    char buf[8];
    struct sockaddr_un a = addr(NAME);
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0 || connect(fd, (struct sockaddr *)&a, sizeof a) < 0) {
        return 1;
    }
    if (write_full(fd, "hello", 5) < 0 || read_full(fd, buf, 5) != 5 || memcmp(buf, "world", 5) != 0) {
        return 2;
    }
    for (int i = 0; i < BULK; i++) {
        bulk[i] = (char)(i * 7);
    }
    if (write_full(fd, bulk, BULK) < 0) {
        return 3;
    }
    close(fd);
    return 0;
}

/* PAIRS socketpairs open, a byte through the last one. */
static int open_pairs(int sv[PAIRS][2]) {
    char c;
    for (int i = 0; i < PAIRS; i++) {
        if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv[i]) < 0) {
            return -1;
        }
    }
    if (write(sv[PAIRS - 1][0], "x", 1) != 1 || read(sv[PAIRS - 1][1], &c, 1) != 1 || c != 'x') {
        return -1;
    }
    return 0;
}

static int many(void) {
    int mine[PAIRS][2], ready[2], go[2], st;
    char c;
    pid_t pid;

    if (pipe(ready) != 0 || pipe(go) != 0) {
        return fail("pipe");
    }
    pid = fork();
    if (pid == 0) {
        int theirs[PAIRS][2];
        c = open_pairs(theirs) == 0 ? 'y' : 'n';
        (void)!write(ready[1], &c, 1);
        (void)!read(go[0], &c, 1);
        _exit(0);
    }
    if (pid < 0 || read(ready[0], &c, 1) != 1 || c != 'y') {
        return fail("a child's socketpairs");
    }
    int ok = open_pairs(mine) == 0;
    (void)!write(go[1], "g", 1);
    waitpid(pid, &st, 0);
    if (!ok) {
        return fail("socketpairs past 32 conversations");
    }
    for (int i = 0; i < PAIRS; i++) {
        close(mine[i][0]);
        close(mine[i][1]);
    }
    close(ready[0]);
    close(ready[1]);
    close(go[0]);
    close(go[1]);
    return 0;
}

/* `echo NAME`: one connection, its bytes back until EOF. */
static int echo(const char *name) {
    char buf[256];
    ssize_t n;
    int ls = listener(name);
    int fd = ls < 0 ? -1 : accept(ls, NULL, NULL);
    if (fd < 0) {
        return fail("echo: listen/accept");
    }
    while ((n = read(fd, buf, sizeof buf)) > 0) {
        if (write_full(fd, buf, (size_t)n) < 0) {
            return fail("echo: write");
        }
    }
    close(fd);
    close(ls);
    return n < 0 ? fail("echo: read") : 0;
}

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

/* Whether `fd` is a SOCK_SEQPACKET socket (SO_TYPE). */
static int is_seqpacket(int fd) {
    int type = 0;
    socklen_t len = sizeof type;
    return getsockopt(fd, SOL_SOCKET, SO_TYPE, &type, &len) == 0 && type == SOCK_SEQPACKET;
}

/* One record of three pieces (sendmsg's iovecs make one). */
static int send3(int fd, const char *a, const char *b, const char *c) {
    struct iovec iov[3] = {{(void *)a, strlen(a)}, {(void *)b, strlen(b)}, {(void *)c, strlen(c)}};
    struct msghdr m;
    memset(&m, 0, sizeof m);
    m.msg_iov = iov;
    m.msg_iovlen = 3;
    return sendmsg(fd, &m, 0) == (ssize_t)(strlen(a) + strlen(b) + strlen(c)) ? 0 : -1;
}

/* `unix_smoke seqchild FD`: the record socket FD, inherited by exec, is one:
 * a record of three pieces goes out whole. */
static int seqchild(const char *arg) {
    int fd = 0;
    while (*arg >= '0' && *arg <= '9') {
        fd = fd * 10 + (*arg++ - '0');
    }
    return is_seqpacket(fd) && send3(fd, "ex", "ec", "'d") == 0 ? 0 : 1;
}

/* Record sockets (SOCK_SEQPACKET, the conversation's `seqpacket` mode):
 * each write is a record read whole by one read, sendmsg's pieces one
 * record; a record a buffer has no room for is cut short (MSG_TRUNC) and
 * the next read starts at the next one; a record over 8 KiB is EMSGSIZE;
 * one that does not fit the peer's free room (64 KiB, eight full records)
 * is refused whole (EAGAIN) until it does; fds come with their record; a named
 * listener takes record clients only; a record socket passed in an
 * SCM_RIGHTS message or inherited by exec is still one. NULL, or what
 * failed. */
static const char *seqpacket(const char *self) {
    static char big[9000];
    char buf[64];
    int sv[2], q[2], fds[4], n, flags, i;
    if (socketpair(AF_UNIX, SOCK_SEQPACKET, 0, sv) != 0 || !is_seqpacket(sv[0]) || !is_seqpacket(sv[1])) {
        return "socketpair";
    }
    if (write(sv[0], "one", 3) != 3 || write(sv[0], "three", 5) != 5 || read(sv[1], buf, sizeof buf) != 3
        || memcmp(buf, "one", 3) != 0 || read(sv[1], buf, sizeof buf) != 5 || memcmp(buf, "three", 5) != 0) {
        return "two writes are two records";
    }
    if (send3(sv[0], "ab", "cd", "ef") != 0 || read(sv[1], buf, sizeof buf) != 6 || memcmp(buf, "abcdef", 6) != 0) {
        return "sendmsg's pieces are one record";
    }
    if (write(sv[0], "0123456789", 10) != 10 || write(sv[0], "next", 4) != 4
        || recv_fds(sv[1], buf, 4, fds, 0, &n, &flags) != 4 || !(flags & MSG_TRUNC)
        || read(sv[1], buf, sizeof buf) != 4 || memcmp(buf, "next", 4) != 0) {
        return "a record cut short (MSG_TRUNC), the next read the next record";
    }
    if (write(sv[0], "fits", 4) != 4 || recv_fds(sv[1], buf, 4, fds, 0, &n, &flags) != 4 || (flags & MSG_TRUNC)) {
        return "a record that fits exactly is no MSG_TRUNC";
    }
    errno = 0;
    if (write(sv[0], big, sizeof big) != -1 || errno != EMSGSIZE) {
        return "a record over 8 KiB: EMSGSIZE";
    }
    fcntl(sv[0], F_SETFL, O_NONBLOCK);
    for (i = 0; i < 8; i++) {
        if (write(sv[0], big, 8192) != 8192) {
            return "eight records of 8 KiB fill the peer's buffer";
        }
    }
    errno = 0;
    if (write(sv[0], big, 8192) != -1 || errno != EAGAIN) {
        return "a record the peer has no room for is refused whole";
    }
    if (read(sv[1], big, sizeof big) != 8192 || write(sv[0], big, 8192) != 8192) {
        return "the record fits once the reader read one";
    }
    for (i = 0; i < 8; i++) {
        if (read(sv[1], big, sizeof big) != 8192) {
            return "each 8 KiB record read whole";
        }
    }
    fcntl(sv[0], F_SETFL, 0);
    /* A record socket in an SCM_RIGHTS message is one: its record of three
     * pieces arrives whole. */
    if (socketpair(AF_UNIX, SOCK_SEQPACKET, 0, q) != 0 || send_fds(sv[0], 'R', &q[1], 1) != 0) {
        return "send a record socket";
    }
    close(q[1]);
    if (recv_fds(sv[1], buf, sizeof buf, fds, 4, &n, &flags) != 1 || buf[0] != 'R' || n != 1
        || !is_seqpacket(fds[0]) || send3(fds[0], "pa", "ss", "ed") != 0 || read(q[0], buf, sizeof buf) != 6
        || memcmp(buf, "passed", 6) != 0) {
        return "a record socket received in SCM_RIGHTS keeps records";
    }
    close(fds[0]);
    /* Inherited by exec: the same. */
    pid_t pid = fork();
    if (pid == 0) {
        char arg[12];
        snprintf(arg, sizeof arg, "%d", sv[0]);
        execl(self, self, "seqchild", arg, (char *)NULL);
        _exit(127);
    }
    int status = 0;
    if (pid < 0 || waitpid(pid, &status, 0) != pid || !WIFEXITED(status) || WEXITSTATUS(status) != 0
        || read(sv[1], buf, sizeof buf) != 6 || memcmp(buf, "exec'd", 6) != 0) {
        return "a record socket inherited by exec keeps records";
    }
    close(q[0]);
    close(sv[0]);
    close(sv[1]);

    /* A named record listener: a stream client is refused, a record one
     * connects and its records arrive one by one. */
    int l = socket(AF_UNIX, SOCK_SEQPACKET, 0);
    struct sockaddr_un a = addr(NAME "-seq");
    if (l < 0 || bind(l, (struct sockaddr *)&a, sizeof a) != 0 || listen(l, 4) != 0) {
        return "a record listener";
    }
    int c = socket(AF_UNIX, SOCK_STREAM, 0);
    if (c < 0 || connect(c, (struct sockaddr *)&a, sizeof a) == 0) {
        return "a stream client of a record listener is refused";
    }
    close(c);
    c = socket(AF_UNIX, SOCK_SEQPACKET, 0);
    if (c < 0 || connect(c, (struct sockaddr *)&a, sizeof a) != 0 || write(c, "hi", 2) != 2 || write(c, "there", 5) != 5) {
        return "a record client connects";
    }
    int s = accept(l, NULL, NULL);
    if (s < 0 || !is_seqpacket(s) || read(s, buf, sizeof buf) != 2 || read(s, buf, sizeof buf) != 5
        || memcmp(buf, "there", 5) != 0) {
        return "the accepted record socket reads record by record";
    }
    close(s);
    close(c);
    close(l);
    return NULL;
}

int main(int argc, char **argv) {
    static char bulk[BULK];
    char buf[8];
    int sv[2];

    if (argc > 2 && strcmp(argv[1], "echo") == 0) {
        return echo(argv[2]);
    }
    if (argc > 2 && strcmp(argv[1], "seqchild") == 0) {
        return seqchild(argv[2]);
    }
    if (argc > 1 && strcmp(argv[1], "seqpacket") == 0) {
        const char *why = seqpacket(argv[0]);
        if (why != NULL) {
            printf("[ FAIL ] unix seqpacket: %s (errno %d)\n", why, errno);
            return 1;
        }
        printf("[ OK ] unix seqpacket\n");
        return 0;
    }

    /* socketpair: both directions, then EOF once one end closes. */
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0) {
        return fail("socketpair");
    }
    if (write(sv[0], "ping", 4) != 4 || read_full(sv[1], buf, 4) != 4 || memcmp(buf, "ping", 4) != 0
        || write(sv[1], "pong", 4) != 4 || read_full(sv[0], buf, 4) != 4 || memcmp(buf, "pong", 4) != 0) {
        return fail("socketpair round trip");
    }
    close(sv[1]);
    if (read(sv[0], buf, sizeof buf) != 0) {
        return fail("socketpair EOF");
    }
    close(sv[0]);

    /* One end holds AHEAD bytes before its reader reads any. */
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0) {
        return fail("socketpair");
    }
    for (int i = 0; i < AHEAD; i++) {
        bulk[i] = (char)(i * 13);
    }
    if (write(sv[0], bulk, AHEAD) != AHEAD) {
        return fail("60000 bytes ahead of the reader");
    }
    for (int i = 0, n; i < AHEAD; i += n) {
        n = read(sv[1], bulk + BULK / 2, AHEAD - i);
        if (n <= 0 || memcmp(bulk + BULK / 2, bulk + i, n) != 0) {
            return fail("60000 bytes read back");
        }
    }
    close(sv[0]);
    close(sv[1]);
    if (many() != 0) {
        return 1;
    }
    const char *e = pass_fds();
    if (e != NULL) {
        return fail(e);
    }

    int ls = listener(NAME);
    if (ls < 0) {
        return fail("listen");
    }
    if (listener(NAME) >= 0 || errno != EADDRINUSE) {
        return fail("second listener not refused");
    }
    struct sockaddr_un none = addr("/tmp/.unix-smoke-nobody");
    int c = socket(AF_UNIX, SOCK_STREAM, 0);
    if (c < 0 || connect(c, (struct sockaddr *)&none, sizeof none) == 0 || errno != ECONNREFUSED) {
        return fail("connect to an unknown name not refused");
    }
    close(c);
    struct sockaddr_un self;
    socklen_t len = sizeof self;
    if (getsockname(ls, (struct sockaddr *)&self, &len) < 0 || strcmp(self.sun_path, NAME) != 0) {
        return fail("getsockname");
    }
    fcntl(ls, F_SETFL, O_NONBLOCK);
    if (accept(ls, NULL, NULL) >= 0 || errno != EAGAIN) {
        return fail("nonblocking accept");
    }
    fcntl(ls, F_SETFL, 0);

    pid_t pid = fork();
    if (pid == 0) {
        _exit(client());
    }
    if (pid < 0) {
        return fail("fork");
    }
    struct pollfd p = {ls, POLLIN, 0};
    if (poll(&p, 1, 60000) != 1 || !(p.revents & POLLIN)) {
        return fail("poll listener");
    }
    int s = accept(ls, NULL, NULL);
    if (s < 0) {
        return fail("accept");
    }
    if (read_full(s, buf, 5) != 5 || memcmp(buf, "hello", 5) != 0 || write_full(s, "world", 5) < 0) {
        return fail("greeting");
    }
    if (read_full(s, bulk, BULK) != BULK) {
        return fail("bulk read");
    }
    for (int i = 0; i < BULK; i++) {
        if (bulk[i] != (char)(i * 7)) {
            return fail("bulk data");
        }
    }
    if (read(s, buf, sizeof buf) != 0) {
        return fail("EOF after the client closed");
    }
    int status;
    if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        printf("[ FAIL ] unix client exit %d\n", WIFEXITED(status) ? WEXITSTATUS(status) : -1);
        return 1;
    }
    close(s);

    /* Closing the listener frees the name. */
    close(ls);
    ls = listener(NAME);
    if (ls < 0) {
        return fail("name free after close");
    }
    close(ls);
    printf("[ OK ] unix\n");
    return 0;
}
