/*
 * Files and fds (kernel/src/task/fd.rs, libgloss syscalls.c): O_CREAT|O_EXCL
 * creates a name only once, even with several processes racing for it, and
 * a symlink there counts as taken; ftruncate cuts a file and grows it with
 * zeros, the position staying; pread and pwrite leave the position alone, a
 * write past the end leaves zeros in the gap, and both are ESPIPE on a pipe;
 * close-on-exec: O_CLOEXEC, F_SETFD, F_DUPFD_CLOEXEC and pipe2 fds are gone
 * in a program the process execs, dup2's copy and the others are not.
 *
 * `fileio_smoke [DIR]` works in DIR (default /tmp): the ext2 test runs it
 * on the scratch disk. `fileio_smoke child FD...` is the exec'd program: FD
 * prefixed with `-` must be closed, plain FD open.
 */
#define _GNU_SOURCE 1 /* pipe2 */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

#define RACERS 4
#define NAMES 40

static int failures;
static char dir[128];

static void check(int ok, const char *what) {
    if (!ok) {
        printf("fileio_smoke: %s (errno %d)\n", what, errno);
        failures++;
    }
}

static const char *in_dir(const char *name) {
    static char path[2][192];
    static int which;
    which ^= 1;
    snprintf(path[which], sizeof path[which], "%s/%s", dir, name);
    return path[which];
}

static int child(int argc, char **argv) {
    int bad = 0;
    for (int i = 2; i < argc; i++) {
        int closed = argv[i][0] == '-';
        int fd = atoi(argv[i] + closed);
        int open_now = fcntl(fd, F_GETFD) >= 0;
        if (open_now == closed) {
            printf("fileio_smoke: fd %d %s after exec\n", fd, closed ? "still open" : "closed");
            bad = 1;
        }
    }
    return bad;
}

static void excl(void) {
    const char *f = in_dir("excl");
    int fd, pfd[2], won = 0;

    unlink(f);
    fd = open(f, O_WRONLY | O_CREAT | O_EXCL, 0644);
    check(fd >= 0, "O_EXCL creates a new file");
    close(fd);
    check(open(f, O_WRONLY | O_CREAT | O_EXCL, 0644) < 0 && errno == EEXIST, "O_EXCL: EEXIST when taken");
    unlink(f);
    /* A symlink at the name is taken, dangling or not; nothing is created
     * where it points. */
    unlink(in_dir("target"));
    check(symlink("target", f) == 0, "symlink");
    check(open(f, O_WRONLY | O_CREAT | O_EXCL, 0644) < 0 && errno == EEXIST, "O_EXCL: a symlink is taken");
    check(access(in_dir("target"), F_OK) < 0, "O_EXCL does not follow the symlink");
    unlink(f);

    /* Racers create the same names: each is won exactly once. */
    for (int i = 0; i < NAMES; i++) {
        char name[16];
        snprintf(name, sizeof name, "race%d", i);
        unlink(in_dir(name));
    }
    check(pipe(pfd) == 0, "pipe");
    for (int r = 0; r < RACERS; r++) {
        if (fork() == 0) {
            unsigned char wins = 0;
            close(pfd[0]);
            for (int i = 0; i < NAMES; i++) {
                char name[16];
                snprintf(name, sizeof name, "race%d", i);
                int f2 = open(in_dir(name), O_WRONLY | O_CREAT | O_EXCL, 0644);
                if (f2 >= 0) {
                    wins++;
                    close(f2);
                }
            }
            write(pfd[1], &wins, 1);
            _exit(0);
        }
    }
    close(pfd[1]);
    for (int r = 0; r < RACERS; r++) {
        unsigned char w = 0;
        check(read(pfd[0], &w, 1) == 1, "racer result");
        won += w;
        wait(NULL);
    }
    close(pfd[0]);
    if (won != NAMES) {
        printf("fileio_smoke: %d racers won %d of %d names\n", RACERS, won, NAMES);
        failures++;
    }
    for (int i = 0; i < NAMES; i++) {
        char name[16];
        snprintf(name, sizeof name, "race%d", i);
        unlink(in_dir(name));
    }
}

static void positional(void) {
    const char *f = in_dir("pos");
    char buf[64];
    struct stat st;
    int fd, pfd[2], zeros = 1;

    fd = open(f, O_RDWR | O_CREAT | O_TRUNC, 0644);
    check(fd >= 0, "open pos");
    check(write(fd, "0123456789", 10) == 10, "write");
    check(pwrite(fd, "ab", 2, 3) == 2, "pwrite");
    check(lseek(fd, 0, SEEK_CUR) == 10, "pwrite leaves the position");
    check(pread(fd, buf, 4, 2) == 4 && memcmp(buf, "2ab5", 4) == 0, "pread");
    check(lseek(fd, 0, SEEK_CUR) == 10, "pread leaves the position");
    check(read(fd, buf, sizeof buf) == 0, "read at the end");
    /* Past the end: zeros in the gap. */
    check(pwrite(fd, "z", 1, 20) == 1, "pwrite past the end");
    check(pread(fd, buf, 11, 10) == 11, "pread the gap");
    for (int i = 0; i < 10; i++) {
        zeros &= buf[i] == 0;
    }
    check(zeros && buf[10] == 'z', "the gap reads as zeros");

    /* ftruncate: cut, then grown with zeros; the position stays. */
    check(ftruncate(fd, 4) == 0, "ftruncate cut");
    check(fstat(fd, &st) == 0 && st.st_size == 4, "size after the cut");
    check(lseek(fd, 0, SEEK_CUR) == 10, "ftruncate leaves the position");
    check(ftruncate(fd, 9000) == 0, "ftruncate grow");
    check(fstat(fd, &st) == 0 && st.st_size == 9000, "size after the growth");
    check(pread(fd, buf, 8, 0) == 8 && memcmp(buf, "012a\0\0\0\0", 8) == 0, "grown with zeros");
    check(pread(fd, buf, 8, 8990) == 8 && memcmp(buf, "\0\0\0\0\0\0\0\0", 8) == 0, "zeros at the end");
    close(fd);
    fd = open(f, O_RDONLY);
    check(ftruncate(fd, 0) < 0, "no ftruncate of a read-only fd");
    close(fd);
    unlink(f);

    check(pipe(pfd) == 0, "pipe");
    check(pwrite(pfd[1], "x", 1, 0) < 0 && errno == ESPIPE, "pwrite on a pipe: ESPIPE");
    check(write(pfd[1], "x", 1) == 1, "write on the pipe");
    check(pread(pfd[0], buf, 1, 0) < 0 && errno == ESPIPE, "pread on a pipe: ESPIPE");
    close(pfd[0]);
    close(pfd[1]);
}

static void cloexec(const char *self) {
    const char *f = in_dir("cloexec");
    int a, b, c, d, e, p[2], status = 0;
    char args[8][8];
    char *argv[10];
    pid_t pid;

    a = open(f, O_RDWR | O_CREAT | O_TRUNC | O_CLOEXEC, 0644);
    b = open(f, O_RDONLY);
    check(a >= 0 && b >= 0, "open cloexec");
    check(fcntl(a, F_GETFD) == FD_CLOEXEC && fcntl(b, F_GETFD) == 0, "F_GETFD");
    c = fcntl(b, F_DUPFD_CLOEXEC, 20);
    check(c >= 20 && fcntl(c, F_GETFD) == FD_CLOEXEC, "F_DUPFD_CLOEXEC");
    d = dup2(a, 30);
    check(d == 30 && fcntl(d, F_GETFD) == 0, "dup2's copy stays open across exec");
    e = open(f, O_RDONLY);
    check(fcntl(e, F_SETFD, FD_CLOEXEC) == 0 && fcntl(e, F_GETFD) == FD_CLOEXEC, "F_SETFD on");
    check(fcntl(b, F_SETFD, FD_CLOEXEC) == 0 && fcntl(b, F_SETFD, 0) == 0 && fcntl(b, F_GETFD) == 0, "F_SETFD off");
    check(pipe2(p, O_CLOEXEC) == 0 && fcntl(p[0], F_GETFD) == FD_CLOEXEC && fcntl(p[1], F_GETFD) == FD_CLOEXEC,
          "pipe2 O_CLOEXEC");

    snprintf(args[0], 8, "-%d", a);
    snprintf(args[1], 8, "%d", b);
    snprintf(args[2], 8, "-%d", c);
    snprintf(args[3], 8, "%d", d);
    snprintf(args[4], 8, "-%d", e);
    snprintf(args[5], 8, "-%d", p[0]);
    snprintf(args[6], 8, "-%d", p[1]);
    argv[0] = (char *)self;
    argv[1] = "child";
    for (int i = 0; i < 7; i++) {
        argv[2 + i] = args[i];
    }
    argv[9] = NULL;
    pid = fork();
    if (pid == 0) {
        execv(self, argv);
        _exit(127);
    }
    check(pid > 0 && waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0,
          "close-on-exec fds are gone after exec, the others are not");
    /* Inherited across fork, not exec. */
    pid = fork();
    if (pid == 0) {
        _exit(fcntl(a, F_GETFD) == FD_CLOEXEC ? 0 : 1);
    }
    check(waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0,
          "fork keeps the flag");
    close(a);
    close(b);
    close(c);
    close(d);
    close(e);
    close(p[0]);
    close(p[1]);
    unlink(f);
}

int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "child") == 0) {
        return child(argc, argv);
    }
    snprintf(dir, sizeof dir, "%s", argc > 1 ? argv[1] : "/tmp");
    excl();
    positional();
    cloexec("/bin/etc/fileio_smoke");
    return failures != 0;
}
