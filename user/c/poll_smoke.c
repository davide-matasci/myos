/* poll-smoke: boot-CI guest test for SYS_POLL (kernel/src/user/syscall.rs,
 * libgloss pollselect.c).
 *
 * A timeout that really elapses; a wait that ends when a child writes to a
 * pipe (not before), with only that fd reported among several; POLLHUP when
 * the writer goes, POLLERR when the reader does, POLLNVAL for a closed fd; a
 * nonblocking read saying EAGAIN; a unix socket and a unix listener waking
 * their poller; a caught signal ending the wait with EINTR. Prints
 * [ OK ] poll.
 */
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <unistd.h>

#define DELAY_MS 300

static pid_t parent;

static int fail(const char *what) {
    printf("[ FAIL ] poll %s (errno %d)\n", what, errno);
    return 1;
}

static long now_ms(void) {
    struct timeval tv;
    gettimeofday(&tv, NULL);
    return tv.tv_sec * 1000L + tv.tv_usec / 1000L;
}

/* A child that sleeps DELAY_MS, then runs `act(arg)` and exits. */
static pid_t later(void (*act)(int), int arg) {
    pid_t pid = fork();
    if (pid == 0) {
        usleep(DELAY_MS * 1000);
        act(arg);
        _exit(0);
    }
    return pid;
}

static void write_byte(int fd) {
    (void)write(fd, "x", 1);
}

static void close_fd(int fd) {
    close(fd);
}

static void connect_to(int unused) {
    struct sockaddr_un a;
    (void)unused;
    memset(&a, 0, sizeof a);
    a.sun_family = AF_UNIX;
    strcpy(a.sun_path, "/tmp/.poll-smoke");
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    (void)connect(fd, (struct sockaddr *)&a, sizeof a);
    usleep(DELAY_MS * 1000); /* keep it open while the parent accepts */
}

static void signal_parent(int sig) {
    kill(parent, sig);
}

static void on_usr1(int sig) {
    (void)sig;
}

/* poll(fds, n, -1) after which `fds[0]` must report `want`, not early. */
static int waits_for(struct pollfd *fds, int n, short want, const char *what) {
    long t0 = now_ms();
    int r = poll(fds, n, 5000);
    long dt = now_ms() - t0;
    if (r < 1 || !(fds[0].revents & want)) {
        printf("[ FAIL ] poll %s: r=%d revents=%#x\n", what, r, fds[0].revents);
        return 1;
    }
    if (dt < DELAY_MS / 2) {
        printf("[ FAIL ] poll %s: woke after %ld ms, before the event\n", what, dt);
        return 1;
    }
    printf("[ INFO ] poll %s after %ld ms\n", what, dt);
    return 0;
}

int main(void) {
    int a[2], b[2], sv[2], status;
    char c;

    parent = getpid();

    /* A timeout that elapses, with nothing ready. */
    if (pipe(a) < 0 || pipe(b) < 0) {
        return fail("pipe");
    }
    struct pollfd p[2] = {{a[0], POLLIN, 0}, {b[0], POLLIN, 0}};
    long t0 = now_ms();
    if (poll(p, 2, 200) != 0 || now_ms() - t0 < 150) {
        return fail("timeout");
    }
    /* A child writes to b: only b is reported. */
    pid_t pid = later(write_byte, b[1]);
    struct pollfd q[2] = {{b[0], POLLIN, 0}, {a[0], POLLIN, 0}};
    if (waits_for(q, 2, POLLIN, "pipe data") || q[1].revents != 0) {
        return fail("pipe data on the right fd only");
    }
    waitpid(pid, &status, 0);
    if (read(b[0], &c, 1) != 1) {
        return fail("read");
    }
    /* POLLOUT now; nonblocking read of the empty pipe: EAGAIN. */
    struct pollfd w = {b[1], POLLOUT, 0};
    if (poll(&w, 1, 0) != 1 || !(w.revents & POLLOUT)) {
        return fail("POLLOUT");
    }
    fcntl(b[0], F_SETFL, O_NONBLOCK);
    if (read(b[0], &c, 1) >= 0 || errno != EAGAIN) {
        return fail("nonblocking read");
    }
    fcntl(b[0], F_SETFL, 0);
    /* The writer goes: POLLHUP. The reader goes: POLLERR. */
    close(a[1]);
    struct pollfd h = {a[0], POLLIN, 0};
    if (poll(&h, 1, 0) != 1 || !(h.revents & POLLHUP)) {
        return fail("POLLHUP");
    }
    close(b[0]);
    w.revents = 0;
    if (poll(&w, 1, 0) != 1 || !(w.revents & POLLERR)) {
        return fail("POLLERR");
    }
    close(a[0]);
    close(b[1]);
    /* A closed fd: POLLNVAL. */
    struct pollfd n = {a[0], POLLIN, 0};
    if (poll(&n, 1, 0) != 1 || !(n.revents & POLLNVAL)) {
        return fail("POLLNVAL");
    }

    /* Unix: data from the peer, then a connection on a listener. */
    if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) < 0) {
        return fail("socketpair");
    }
    pid = later(write_byte, sv[1]);
    struct pollfd u = {sv[0], POLLIN, 0};
    if (waits_for(&u, 1, POLLIN, "unix data")) {
        return 1;
    }
    waitpid(pid, &status, 0);
    close(sv[0]);
    close(sv[1]);
    struct sockaddr_un addr;
    memset(&addr, 0, sizeof addr);
    addr.sun_family = AF_UNIX;
    strcpy(addr.sun_path, "/tmp/.poll-smoke");
    int ls = socket(AF_UNIX, SOCK_STREAM, 0);
    if (ls < 0 || bind(ls, (struct sockaddr *)&addr, sizeof addr) < 0 || listen(ls, 1) < 0) {
        return fail("listen");
    }
    pid = later(connect_to, 0);
    struct pollfd l = {ls, POLLIN, 0};
    if (waits_for(&l, 1, POLLIN, "unix connection")) {
        return 1;
    }
    int s = accept(ls, NULL, NULL);
    if (s < 0) {
        return fail("accept");
    }
    waitpid(pid, &status, 0);
    close(s);
    close(ls);

    /* A caught signal ends an endless wait. */
    struct sigaction sa;
    memset(&sa, 0, sizeof sa);
    sa.sa_handler = on_usr1;
    sigaction(SIGUSR1, &sa, NULL);
    if (pipe(a) < 0) {
        return fail("pipe");
    }
    pid = later(signal_parent, SIGUSR1);
    struct pollfd e = {a[0], POLLIN, 0};
    errno = 0;
    if (poll(&e, 1, -1) != -1 || errno != EINTR) {
        return fail("EINTR");
    }
    waitpid(pid, &status, 0);
    printf("[ OK ] poll\n");
    return 0;
}
