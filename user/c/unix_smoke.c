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
 */
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
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

int main(void) {
    static char bulk[BULK];
    char buf[8];
    int sv[2];

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
