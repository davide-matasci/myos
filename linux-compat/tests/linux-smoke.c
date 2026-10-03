/*
 * Boot smoke for the optional Linux compatibility layer. Built as a static-PIE
 * Linux binary against musl (not myos newlib) for each arch and run as
 * `linux linux-smoke`. Prints "LINUX-SMOKE OK" when every check passes.
 *
 * No printf: musl's printf needs the compiler runtime's quad-float helpers
 * on aarch64/riscv64, which this build does not ship.
 */
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/utsname.h>
#include <sys/wait.h>
#include <termios.h>
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

int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "child") == 0) {
        return 5;
    }

    struct utsname u;
    check(uname(&u) == 0 && strcmp(u.sysname, "Linux") == 0, "uname");

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
    check(kill(c, SIGUSR1) == 0, "kill");
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

    /* The terminal: the boot test runs this with stdout in a file and stdin
     * on the console. musl's tty calls are the Linux ioctls, which the layer
     * serves from the terminal's ctl file (docs/tty.md). */
    struct termios t;
    struct winsize ws;
    char link[64];
    ssize_t ln = readlink("/proc/self/fd/0", link, sizeof link - 1);
    check(isatty(0) == 1 && isatty(1) == 0, "isatty");
    check(tcgetattr(0, &t) == 0 && (t.c_lflag & ICANON) && t.c_cc[VINTR] == 3, "tcgetattr");
    check(ioctl(0, TIOCGWINSZ, &ws) == 0 && ws.ws_row > 0 && ws.ws_col > 0, "TIOCGWINSZ");
    check(ln > 5 && strcmp(link + ln - 5, "/data") == 0, "fd link names the terminal");
    check(tcgetattr(1, &t) == -1 && errno == ENOTTY, "tcgetattr on a file: ENOTTY");

    if (failures) {
        out("LINUX-SMOKE FAIL\n");
        return 1;
    }
    out("LINUX-SMOKE OK\n");
    return 0;
}
