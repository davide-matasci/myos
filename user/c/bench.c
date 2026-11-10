/*
 * bench: micro-benchmarks of the kernel's common paths, to measure a
 * change against the tree before it (docs/testing.md, "Measuring"). Not a
 * test: the numbers are a machine's and an emulator's, and say something
 * only next to those of another kernel on the same one.
 *
 *   bench [SCALE [NAME...]]
 *
 * Without names, two rounds of everything: a bare syscall, 1-byte and
 * 16 KiB reads and writes of a tmpfs file, a 1-byte write to /dev/null,
 * stat, open+close, a byte back and forth through two pipes, a stream
 * through a pipe, fork+exit+wait, mmap and touch, a stat/open/read loop in
 * one process and in three at once (what the kernel's global locks cost),
 * and a stream over TCP on 127.0.0.1. With names (stat open null pwrite16
 * pingpong stream tcp fork mmap getppid par1 par3), those, once each, for as long as
 * a profile needs (scripts/prof-sample.py): SCALE multiplies the counts.
 *
 * A line per result, `BENCH name ... ns/op` or `ms`, on fd 3 when it is
 * open (the boot tests' console, user/tests/run.sh), else on stdout.
 */
#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <signal.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

static int out = 1;

static uint64_t now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * 1000000000ull + ts.tv_nsec;
}

static void report(const char *name, uint64_t ns, uint64_t n, const char *unit) {
    char line[160];
    int len;
    if (unit)
        len = snprintf(line, sizeof line, "BENCH %-18s %8llu ms  %s\n", name, (unsigned long long)(ns / 1000000), unit);
    else
        len = snprintf(line, sizeof line, "BENCH %-18s %8llu ns/op (%llu ops)\n", name, (unsigned long long)(ns / n),
                       (unsigned long long)n);
    write(out, line, len);
}

static void b_getppid(int n) {
    uint64_t t = now_ns();
    for (int i = 0; i < n; i++) getppid();
    report("getppid", now_ns() - t, n, NULL);
}

static void b_small_io(int n) {
    char c = 'x';
    int fd = open("/tmp/bench.dat", O_RDWR | O_CREAT | O_TRUNC, 0600);
    static char blk[16384];
    memset(blk, 'a', sizeof blk);
    for (int i = 0; i < 64; i++) write(fd, blk, sizeof blk);
    uint64_t t = now_ns();
    for (int i = 0; i < n; i++) pread(fd, &c, 1, 100);
    report("pread 1B tmpfs", now_ns() - t, n, NULL);
    t = now_ns();
    for (int i = 0; i < n; i++) pwrite(fd, &c, 1, 100);
    report("pwrite 1B tmpfs", now_ns() - t, n, NULL);
    t = now_ns();
    int m = n / 8;
    for (int i = 0; i < m; i++) pread(fd, blk, sizeof blk, (i % 64) * 16384);
    report("pread 16K tmpfs", now_ns() - t, m, NULL);
    t = now_ns();
    for (int i = 0; i < m; i++) pwrite(fd, blk, sizeof blk, (i % 64) * 16384);
    report("pwrite 16K tmpfs", now_ns() - t, m, NULL);
    close(fd);
    int nul = open("/dev/null", O_WRONLY);
    if (nul >= 0) {
        t = now_ns();
        for (int i = 0; i < n; i++) write(nul, &c, 1);
        report("write 1B null", now_ns() - t, n, NULL);
        close(nul);
    }
}

static void b_paths(int n) {
    struct stat st;
    uint64_t t = now_ns();
    for (int i = 0; i < n; i++) stat("/tmp/bench.dat", &st);
    report("stat", now_ns() - t, n, NULL);
    t = now_ns();
    for (int i = 0; i < n; i++) close(open("/tmp/bench.dat", O_RDONLY));
    report("open+close", now_ns() - t, n, NULL);
}

static void b_pingpong(int n) {
    int a[2], b[2];
    char c = 0;
    pipe(a);
    pipe(b);
    pid_t pid = fork();
    if (pid == 0) {
        close(a[1]);
        close(b[0]);
        while (read(a[0], &c, 1) == 1) write(b[1], &c, 1);
        _exit(0);
    }
    close(a[0]);
    close(b[1]);
    uint64_t t = now_ns();
    for (int i = 0; i < n; i++) {
        write(a[1], &c, 1);
        read(b[0], &c, 1);
    }
    report("pipe ping-pong", now_ns() - t, n, NULL);
    close(a[1]);
    close(b[0]);
    waitpid(pid, NULL, 0);
}

static void b_pipe_stream(int mib) {
    int p[2];
    static char blk[65536];
    pipe(p);
    pid_t pid = fork();
    if (pid == 0) {
        close(p[0]);
        memset(blk, 'z', sizeof blk);
        for (int i = 0; i < mib * 16; i++) {
            size_t off = 0;
            while (off < sizeof blk) {
                ssize_t w = write(p[1], blk + off, sizeof blk - off);
                if (w <= 0) _exit(1);
                off += w;
            }
        }
        _exit(0);
    }
    close(p[1]);
    uint64_t t = now_ns(), total = 0;
    ssize_t r;
    while ((r = read(p[0], blk, sizeof blk)) > 0) total += r;
    char unit[64];
    snprintf(unit, sizeof unit, "(%llu MiB)", (unsigned long long)(total >> 20));
    report("pipe stream", now_ns() - t, 1, unit);
    close(p[0]);
    waitpid(pid, NULL, 0);
}

static void b_fork(int n) {
    uint64_t t = now_ns();
    for (int i = 0; i < n; i++) {
        pid_t pid = fork();
        if (pid == 0) _exit(0);
        waitpid(pid, NULL, 0);
    }
    report("fork+exit+wait", now_ns() - t, n, NULL);
}

static void b_mmap(int n) {
    size_t len = 64 * 4096;
    uint64_t t = now_ns();
    for (int i = 0; i < n; i++) {
        char *p = mmap(NULL, len, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
        if (p == MAP_FAILED) break;
        for (size_t o = 0; o < len; o += 4096) p[o] = 1;
        munmap(p, len);
    }
    report("mmap+touch 64p", now_ns() - t, n, NULL);
}

/* The same path loop in `procs` processes at once. */
static void b_parallel(int procs, int n) {
    uint64_t t = now_ns();
    for (int p = 0; p < procs; p++) {
        if (fork() == 0) {
            struct stat st;
            char c;
            int fd = open("/tmp/bench.dat", O_RDONLY);
            for (int i = 0; i < n; i++) {
                stat("/tmp/bench.dat", &st);
                close(open("/tmp/bench.dat", O_RDONLY));
                pread(fd, &c, 1, 0);
            }
            _exit(0);
        }
    }
    for (int p = 0; p < procs; p++) wait(NULL);
    char name[32];
    snprintf(name, sizeof name, "parallel x%d", procs);
    report(name, now_ns() - t, n, NULL);
}

static void b_tcp(int mib) {
    int ls = socket(AF_INET, SOCK_STREAM, 0);
    struct sockaddr_in sa = {0};
    sa.sin_family = AF_INET;
    sa.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    sa.sin_port = htons(45123);
    if (ls < 0 || bind(ls, (struct sockaddr *)&sa, sizeof sa) < 0 || listen(ls, 1) < 0) {
        write(out, "BENCH tcp: no listener\n", 23);
        return;
    }
    static char blk[16384];
    pid_t pid = fork();
    if (pid == 0) {
        int s = socket(AF_INET, SOCK_STREAM, 0);
        if (connect(s, (struct sockaddr *)&sa, sizeof sa) < 0) _exit(1);
        memset(blk, 'n', sizeof blk);
        for (int i = 0; i < mib * 64; i++) {
            size_t off = 0;
            while (off < sizeof blk) {
                ssize_t w = write(s, blk + off, sizeof blk - off);
                if (w <= 0) _exit(1);
                off += w;
            }
        }
        close(s);
        _exit(0);
    }
    int s = accept(ls, NULL, NULL);
    uint64_t t = now_ns(), total = 0;
    ssize_t r;
    while ((r = read(s, blk, sizeof blk)) > 0) total += r;
    char unit[64];
    snprintf(unit, sizeof unit, "(%llu KiB)", (unsigned long long)(total >> 10));
    report("tcp loopback", now_ns() - t, 1, unit);
    close(s);
    close(ls);
    waitpid(pid, NULL, 0);
}

static void b_stat(int n) {
    struct stat st;
    uint64_t t = now_ns();
    for (int i = 0; i < n; i++) stat("/tmp/bench.dat", &st);
    report("stat", now_ns() - t, n, NULL);
}
static void b_open(int n) {
    uint64_t t = now_ns();
    for (int i = 0; i < n; i++) close(open("/tmp/bench.dat", O_RDONLY));
    report("open+close", now_ns() - t, n, NULL);
}
static void b_null(int n) {
    char c = 'x';
    int nul = open("/dev/null", O_WRONLY);
    uint64_t t = now_ns();
    for (int i = 0; i < n; i++) write(nul, &c, 1);
    report("write 1B null", now_ns() - t, n, NULL);
    close(nul);
}
static void b_pwrite16(int n) {
    static char blk[16384];
    int fd = open("/tmp/bench.dat", O_RDWR);
    uint64_t t = now_ns();
    for (int i = 0; i < n; i++) pwrite(fd, blk, sizeof blk, (i % 64) * 16384);
    report("pwrite 16K tmpfs", now_ns() - t, n, NULL);
    close(fd);
}

int main(int argc, char **argv) {
    if (fcntl(3, F_GETFD) >= 0) out = 3;
    int scale = argc > 1 ? atoi(argv[1]) : 1;
    if (scale < 1) scale = 1;
    if (argc > 2) {
        b_small_io(1000);
        write(out, "BENCH begin\n", 12);
        for (int a = 2; a < argc; a++) {
            const char *w = argv[a];
            if (!strcmp(w, "stat")) b_stat(20000 * scale);
            else if (!strcmp(w, "open")) b_open(20000 * scale);
            else if (!strcmp(w, "null")) b_null(40000 * scale);
            else if (!strcmp(w, "pwrite16")) b_pwrite16(5000 * scale);
            else if (!strcmp(w, "pingpong")) b_pingpong(10000 * scale);
            else if (!strcmp(w, "stream")) b_pipe_stream(32 * scale);
            else if (!strcmp(w, "tcp")) b_tcp(4 * scale);
            else if (!strcmp(w, "fork")) b_fork(200 * scale);
            else if (!strcmp(w, "mmap")) b_mmap(300 * scale);
            else if (!strcmp(w, "getppid")) b_getppid(100000 * scale);
            else if (!strcmp(w, "par1")) b_parallel(1, 10000 * scale);
            else if (!strcmp(w, "par3")) b_parallel(3, 10000 * scale);
        }
        return 0;
    }

    for (int round = 0; round < 2; round++) {
        b_getppid(100000 * scale);
        b_small_io(40000 * scale);
        b_paths(20000 * scale);
        b_pingpong(10000 * scale);
        b_pipe_stream(32 * scale);
        b_fork(200 * scale);
        b_mmap(300 * scale);
        b_parallel(1, 10000 * scale);
        b_parallel(3, 10000 * scale);
        b_tcp(4 * scale);
    }
    unlink("/tmp/bench.dat");
    return 0;
}
