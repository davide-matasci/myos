/* The libc functions libgloss gained for the ports' shims to go
 * (toolchain/newlib/libgloss/myos/posix_extra.c, netdb.c): getrandom from
 * /dev/urandom, vfork, daemon, the service lookups that find nothing, and the termios and netinet constants the headers now carry.
 * Prints one `[ OK ] libc` or a `[ FAIL ] libc ...` line; the boot test
 * reads the exit status. */
#include <errno.h>
#include <fcntl.h>
#include <netdb.h>
#include <netinet/in.h>
#include <stdio.h>
#include <string.h>
#include <sys/random.h>
#include <sys/stat.h>
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
 * forked once more). The session is not checked: libgloss's setsid is
 * still a no-op (posix_stubs.c), so daemon's process keeps its parent's. */
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
    if (n != 2 || dpid == (int)pid || dpid <= 0) {
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
    return IPPORT_RESERVED == 1024 ? 0 : fail("IPPORT_RESERVED");
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

int main(void) {
    if (check_getrandom() || check_vfork() || check_daemon() || check_netdb()
        || check_termios()) {
        return 1;
    }
    printf("[ OK ] libc\n");
    return 0;
}
