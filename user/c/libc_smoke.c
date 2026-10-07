/* The libc functions libgloss gained for the ports' shims to go
 * (toolchain/newlib/libgloss/myos/posix_extra.c, netdb.c): getrandom from
 * /dev/urandom, vfork, daemon, the service lookups that find nothing, the
 * resolver (localhost, a missing name failing in bounded time), and the
 * termios and netinet constants the headers now carry;
 * the interval timer and alarm (SIGALRM on time, a blocking read cut short,
 * the default action).
 * Prints one `[ OK ] libc` or a `[ FAIL ] libc ...` line; the boot test
 * reads the exit status. */
#include <errno.h>
#include <fcntl.h>
#include <netdb.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/random.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/wait.h>
#include <termios.h>
#include <time.h>
#include <unistd.h>

static int fail(const char *what) {
    printf("[ FAIL ] libc %s (errno %d)\n", what, errno);
    return 1;
}

static int check_getrandom(void) {
    unsigned char a[32], b[32];
    memset(a, 0, sizeof a);
    memset(b, 0, sizeof b);
    if (getrandom(a, sizeof a, 0) != (ssize_t)sizeof a) {
        return fail("getrandom");
    }
    if (getrandom(b, sizeof b, GRND_NONBLOCK) != (ssize_t)sizeof b) {
        return fail("getrandom GRND_NONBLOCK");
    }
    int zero = 1;
    for (size_t i = 0; i < sizeof a; i++) {
        if (a[i] != 0) {
            zero = 0;
        }
    }
    if (zero || memcmp(a, b, sizeof a) == 0) {
        return fail("getrandom bytes");
    }
    errno = 0;
    if (getrandom(a, sizeof a, 0x80) != -1 || errno != EINVAL) {
        return fail("getrandom bad flags");
    }
    return 0;
}

static int check_vfork(void) {
    pid_t pid = vfork();
    if (pid < 0) {
        return fail("vfork");
    }
    if (pid == 0) {
        _exit(7);
    }
    int status = 0;
    if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status) || WEXITSTATUS(status) != 7) {
        return fail("vfork child");
    }
    return 0;
}

/* A child daemonizes with its descriptors kept, then writes its pid and
 * session id to a file: a different pid from the child we forked (daemon
 * forked once more), leading a session of its own. */
static int check_daemon(void) {
    const char *path = "/tmp/libc-smoke-daemon";
    unlink(path);
    pid_t pid = fork();
    if (pid < 0) {
        return fail("fork");
    }
    if (pid == 0) {
        if (daemon(1, 1) != 0) {
            _exit(1);
        }
        /* Written under another name and renamed: the parent polls for
         * the file and must not read it half-written. */
        FILE *f = fopen("/tmp/libc-smoke-daemon.tmp", "w");
        if (f == NULL) {
            _exit(2);
        }
        fprintf(f, "%d %d\n", (int)getpid(), (int)getsid(0));
        fclose(f);
        if (rename("/tmp/libc-smoke-daemon.tmp", path) != 0) {
            _exit(3);
        }
        _exit(0);
    }
    int status = 0;
    if (waitpid(pid, &status, 0) != pid || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        return fail("daemon parent exit");
    }
    FILE *f = NULL;
    for (int i = 0; i < 300 && f == NULL; i++) {
        f = fopen(path, "r");
        if (f == NULL) {
            struct timespec ts = { 0, 10 * 1000 * 1000 };
            nanosleep(&ts, NULL);
        }
    }
    if (f == NULL) {
        return fail("daemon never wrote");
    }
    int dpid = 0, dsid = 0;
    int n = fscanf(f, "%d %d", &dpid, &dsid);
    fclose(f);
    unlink(path);
    if (n != 2 || dpid == (int)pid || dpid <= 0 || dsid != dpid) {
        printf("[ FAIL ] libc daemon: pid %d sid %d (forked %d)\n", dpid, dsid, (int)pid);
        return 1;
    }
    return 0;
}

static int check_netdb(void) {
    if (getservbyname("http", "tcp") != NULL || getservbyport(htons(80), "tcp") != NULL) {
        return fail("getservbyname found a service");
    }
    if (hstrerror(HOST_NOT_FOUND) == NULL || strlen(hstrerror(NO_DATA)) == 0) {
        return fail("hstrerror");
    }
    h_errno = TRY_AGAIN;
    if (h_errno != TRY_AGAIN) {
        return fail("h_errno");
    }
    struct in6_addr lo = IN6ADDR_LOOPBACK_INIT;
    struct in6_addr any = IN6ADDR_ANY_INIT;
    if (!IN6_IS_ADDR_LOOPBACK(&lo) || IN6_IS_ADDR_LOOPBACK(&any) || !IN6_IS_ADDR_UNSPECIFIED(&any)
        || IN6_IS_ADDR_V4MAPPED(&lo)) {
        return fail("IN6_IS_ADDR");
    }
    char host[NI_MAXHOST];
    (void)host;
    if (IPPORT_RESERVED != 1024) {
        return fail("IPPORT_RESERVED");
    }
    /* localhost resolves without a lookup; a name no server knows fails,
     * and in bounded time: the resolver used to read its socket 400000
     * times over instead of waiting for the answer (an X client falling
     * back to localhost:6000 spun for minutes on it). */
    struct addrinfo hints;
    struct addrinfo *res = NULL;
    memset(&hints, 0, sizeof hints);
    hints.ai_family = AF_INET;
    hints.ai_socktype = SOCK_STREAM;
    if (getaddrinfo("localhost", NULL, &hints, &res) != 0 || res == NULL
        || ((struct sockaddr_in *)res->ai_addr)->sin_addr.s_addr != htonl(INADDR_LOOPBACK)) {
        return fail("getaddrinfo localhost");
    }
    freeaddrinfo(res);
    struct timeval t0, t1;
    gettimeofday(&t0, NULL);
    int rc = getaddrinfo("nonexistent.invalid", NULL, &hints, &res);
    gettimeofday(&t1, NULL);
    long ms = (t1.tv_sec - t0.tv_sec) * 1000L + (t1.tv_usec - t0.tv_usec) / 1000L;
    if (rc == 0) {
        freeaddrinfo(res);
        return fail("nonexistent.invalid resolved");
    }
    if (ms > 10000) {
        printf("[ FAIL ] libc getaddrinfo of a missing name took %ld ms\n", ms);
        return 1;
    }
    return 0;
}

static int check_termios(void) {
    /* The constants exist and fit the fields; the kernel ignores them. */
    struct termios t;
    memset(&t, 0, sizeof t);
    t.c_iflag |= IXANY;
    t.c_cflag |= PARENB | PARODD | CS7;
    t.c_cc[VSTART] = 021;
    t.c_cc[VSTOP] = 023;
    t.c_cc[VEOL] = 0;
    return (t.c_cflag & CSIZE) == CS7 && VSTART < NCCS && VEOL < NCCS ? 0 : fail("termios");
}

/* The soft float of the arches without an FPU in the image (riscv64:
 * compiler-rt's helpers, the long-double conversions of
 * ports/sbase/riscv64-softfloat.c): arithmetic, compares, the integer
 * conversions, and printf and strtod through them. Hand-written helpers
 * once made 80 + 100 give 116 and a == a false. volatile keeps the
 * compiler from folding the arithmetic at build time. */
static int check_double(void) {
    volatile double a = 80.0, b = 100.0, c = 1000.0, nan = 0.0;
    volatile float f = 1.5f;
    char buf[64];
    nan = nan / nan;
    if (a + b != 180.0 || a * c != 80000.0 || (a + b) / 8 != 22.5 || b - c != -900.0) {
        return fail("double arithmetic");
    }
    if (!(a == a) || a == b || !(a < b) || !(b >= a) || a > b || !(a <= a) || !(a != b)) {
        return fail("double compares");
    }
    if (nan == nan || nan < a || nan > a || !(nan != nan)) {
        return fail("nan compares");
    }
    if ((int)(a * c) != 80000 || (long long)-(a + b) != -180 || (unsigned)(b / 8) != 12
        || (double)(int)-7 != -7.0 || (double)4000000000ULL != 4e9) {
        return fail("double conversions");
    }
    if (f * 2 != 3.0f || !(f < 2.0f) || (double)f != 1.5) {
        return fail("float arithmetic");
    }
    snprintf(buf, sizeof buf, "%.1f %g %.3f", (double)(a + b), (double)(a * c), (double)(a + b) / 8);
    if (strcmp(buf, "180.0 80000 22.500") != 0) {
        printf("printf: %s\n", buf);
        return fail("double printf");
    }
    if (strtod("2.5e2", NULL) != 250.0 || atof("0.125") * 8 != 1.0) {
        return fail("strtod");
    }
    return 0;
}

static volatile sig_atomic_t alarms;

static void on_alarm(int sig) {
    (void)sig;
    alarms++;
}

static int check_timers(void) {
    struct itimerval it = {{0, 50000}, {0, 50000}}, left;
    struct sigaction sa;
    int fds[2], st, i, n;
    char c;
    pid_t pid;

    memset(&sa, 0, sizeof sa);
    sa.sa_handler = on_alarm;
    sigemptyset(&sa.sa_mask);
    if (sigaction(SIGALRM, &sa, NULL) != 0) {
        return fail("sigaction SIGALRM");
    }
    /* Every 50 ms: three within a couple of seconds whatever the load. */
    if (setitimer(ITIMER_REAL, &it, NULL) != 0) {
        return fail("setitimer");
    }
    for (i = 0; i < 200 && alarms < 3; i++) {
        usleep(10000);
    }
    if (alarms < 3) {
        return fail("SIGALRM every 50 ms");
    }
    if (getitimer(ITIMER_REAL, &left) != 0 || left.it_interval.tv_sec != 0 || left.it_interval.tv_usec != 50000
        || left.it_value.tv_sec != 0 || left.it_value.tv_usec > 50000) {
        return fail("getitimer");
    }
    memset(&it, 0, sizeof it);
    if (setitimer(ITIMER_REAL, &it, NULL) != 0) {
        return fail("setitimer to disarm");
    }
    n = alarms;
    usleep(200000);
    if (alarms != n) {
        return fail("a disarmed timer fired");
    }
    /* alarm: the seconds left of the one before, rounded up. */
    if (alarm(5) != 0 || alarm(1) != 5) {
        return fail("alarm's seconds left");
    }
    if (pipe(fds) != 0) {
        return fail("pipe");
    }
    errno = 0;
    if (read(fds[0], &c, 1) != -1 || errno != EINTR || alarms != n + 1) {
        return fail("SIGALRM ends a blocking read");
    }
    close(fds[0]);
    close(fds[1]);
    /* Uncaught, SIGALRM ends the process. */
    pid = fork();
    if (pid == 0) {
        signal(SIGALRM, SIG_DFL);
        alarm(1);
        for (i = 0; i < 50; i++) {
            usleep(100000);
        }
        _exit(0);
    }
    if (pid < 0 || waitpid(pid, &st, 0) != pid || !WIFSIGNALED(st) || WTERMSIG(st) != SIGALRM) {
        return fail("SIGALRM's default action");
    }
    return 0;
}

int main(void) {
    if (check_getrandom() || check_vfork() || check_daemon() || check_netdb()
        || check_termios() || check_double() || check_timers()) {
        return 1;
    }
    printf("[ OK ] libc\n");
    return 0;
}
