/*
 * Files and fds (kernel/src/task/fd.rs, libgloss syscalls.c): O_CREAT|O_EXCL
 * creates a name only once, even with several processes racing for it, and
 * a symlink there counts as taken; ftruncate cuts a file and grows it with
 * zeros, the position staying; pread and pwrite leave the position alone, a
 * write past the end leaves zeros in the gap, and both are ESPIPE on a pipe;
 * close-on-exec: O_CLOEXEC, F_SETFD, F_DUPFD_CLOEXEC and pipe2 fds are gone
 * in a program the process execs, dup2's copy and the others are not;
 * O_NOFOLLOW refuses a symlink (ELOOP) and O_DIRECTORY anything but a
 * directory (ENOTDIR), and a directory opened to write, create or truncate
 * is EISDIR; flock and fcntl record locks (kernel/src/fs/lock.rs) conflict
 * across processes, wait, and go with their owner; a file is its inode, not
 * its name: an fd follows it through a rename and keeps it after an unlink,
 * and its inode number stays; a read of a file gives all that was asked
 * for, up to its end, in one call (not a 4 KiB chunk: fontconfig took that
 * for a damaged cache); a rename does not wait for a process reading the
 * console (which held the filesystem tree until the next key).
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
#include <signal.h>
#include <sys/file.h>
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

static void open_flags(void) {
    const char *f = in_dir("nofollow"), *l = in_dir("nofollow-link");
    int fd;

    unlink(l);
    close(open(f, O_WRONLY | O_CREAT | O_TRUNC, 0644));
    check(symlink("nofollow", l) == 0, "symlink");
    check(open(l, O_RDONLY | O_NOFOLLOW) < 0 && errno == ELOOP, "O_NOFOLLOW on a symlink: ELOOP");
    fd = open(f, O_RDONLY | O_NOFOLLOW);
    check(fd >= 0, "O_NOFOLLOW on a file");
    close(fd);
    fd = open(l, O_RDONLY);
    check(fd >= 0, "a symlink is followed without O_NOFOLLOW");
    close(fd);
    check(open(f, O_RDONLY | O_DIRECTORY) < 0 && errno == ENOTDIR, "O_DIRECTORY on a file: ENOTDIR");
    fd = open(dir, O_RDONLY | O_DIRECTORY);
    check(fd >= 0, "O_DIRECTORY on a directory");
    close(fd);
    /* A directory is not opened to write, create or truncate (EISDIR). */
    check(open(dir, O_WRONLY) < 0 && errno == EISDIR, "a directory for writing: EISDIR");
    check(open(dir, O_RDWR | O_CREAT, 0644) < 0 && errno == EISDIR, "O_CREAT of a directory: EISDIR");
    check(open(dir, O_RDONLY | O_TRUNC) < 0 && errno == EISDIR, "O_TRUNC of a directory: EISDIR");
    unlink(l);
    unlink(f);
}

/* Run `test` in a child (which has its own locks, and fds of its own
 * description when it opens the file again): its exit status. */
static int in_child(int (*test)(const char *), const char *f) {
    int status = 0;
    pid_t pid = fork();
    if (pid == 0) {
        _exit(test(f));
    }
    return pid > 0 && waitpid(pid, &status, 0) == pid && WIFEXITED(status) ? WEXITSTATUS(status) : 99;
}

static int try_flock_ex(const char *f) {
    int fd = open(f, O_RDONLY);
    return flock(fd, LOCK_EX | LOCK_NB) == 0 ? 0 : errno == EWOULDBLOCK ? 1 : 2;
}

static int try_flock_sh(const char *f) {
    int fd = open(f, O_RDONLY);
    return flock(fd, LOCK_SH | LOCK_NB) == 0 ? 0 : errno == EWOULDBLOCK ? 1 : 2;
}

static int try_record(const char *f, int type, off_t start, off_t len) {
    int fd = open(f, O_RDWR);
    struct flock fl = {.l_type = type, .l_whence = SEEK_SET, .l_start = start, .l_len = len};
    return fcntl(fd, F_SETLK, &fl) == 0 ? 0 : errno == EAGAIN ? 1 : 2;
}

static int try_bytes_0_9(const char *f) { return try_record(f, F_WRLCK, 0, 10); }
static int try_bytes_10_19(const char *f) { return try_record(f, F_WRLCK, 10, 10); }
static int try_bytes_100_on(const char *f) { return try_record(f, F_WRLCK, 100, 0); }
static int try_read_0_9(const char *f) { return try_record(f, F_RDLCK, 0, 10); }

static int get_holder(const char *f) {
    int fd = open(f, O_RDWR);
    struct flock fl = {.l_type = F_WRLCK, .l_whence = SEEK_SET, .l_start = 0, .l_len = 0};
    if (fcntl(fd, F_GETLK, &fl) < 0) {
        return 2;
    }
    /* The parent's lock on 0..10, its pid. */
    return fl.l_type == F_WRLCK && fl.l_start == 0 && fl.l_len == 10 && fl.l_pid == (short)getppid() ? 0 : 1;
}

static void on_signal(int sig) {
    (void)sig;
}

static void locks(void) {
    const char *f = in_dir("locks");
    int fd, fd2, pfd[2], status = 0;
    struct flock fl;
    pid_t pid;
    char c;

    fd = open(f, O_RDWR | O_CREAT | O_TRUNC, 0644);
    check(fd >= 0, "open locks");

    /* flock: of the description, whole file. */
    check(flock(fd, LOCK_EX) == 0, "flock LOCK_EX");
    check(in_child(try_flock_ex, f) == 1, "flock: another description's LOCK_EX is refused");
    check(in_child(try_flock_sh, f) == 1, "flock: and its LOCK_SH");
    check(flock(fd, LOCK_SH) == 0, "flock: LOCK_EX to LOCK_SH");
    check(in_child(try_flock_sh, f) == 0, "flock: shared with a LOCK_SH");
    check(in_child(try_flock_ex, f) == 1, "flock: a LOCK_SH keeps a LOCK_EX out");
    check(flock(fd, LOCK_UN) == 0 && in_child(try_flock_ex, f) == 0, "flock: LOCK_UN lets go");
    /* A dup and a fork share the description, and with it the lock: it
     * goes with the description's last fd. */
    check(flock(fd, LOCK_EX) == 0, "flock again");
    fd2 = dup(fd);
    close(fd);
    check(in_child(try_flock_ex, f) == 1, "flock: held while a dup is open");
    close(fd2);
    check(in_child(try_flock_ex, f) == 0, "flock: gone with the description's last fd");

    /* A blocking flock waits until the holder lets go. */
    fd = open(f, O_RDWR);
    check(flock(fd, LOCK_EX) == 0, "flock for the waiter");
    check(pipe(pfd) == 0, "pipe");
    pid = fork();
    if (pid == 0) {
        int mine = open(f, O_RDONLY);
        close(pfd[0]);
        write(pfd[1], "w", 1);
        _exit(flock(mine, LOCK_EX) == 0 ? 0 : 1);
    }
    close(pfd[1]);
    check(read(pfd[0], &c, 1) == 1, "the waiter started");
    close(pfd[0]);
    check(waitpid(pid, &status, WNOHANG) == 0, "flock: the waiter waits");
    check(flock(fd, LOCK_UN) == 0, "flock: let the waiter in");
    check(waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0,
          "flock: the waiter got the lock");
    /* A caught signal ends the wait (EINTR). */
    check(flock(fd, LOCK_EX) == 0, "flock for the interrupted waiter");
    check(pipe(pfd) == 0, "pipe");
    pid = fork();
    if (pid == 0) {
        struct sigaction sa = {0};
        int mine = open(f, O_RDONLY);
        sa.sa_handler = on_signal;
        sigaction(SIGUSR1, &sa, NULL);
        close(pfd[0]);
        write(pfd[1], "w", 1);
        _exit(flock(mine, LOCK_EX) < 0 && errno == EINTR ? 0 : 1);
    }
    close(pfd[1]);
    check(read(pfd[0], &c, 1) == 1, "the interrupted waiter started");
    close(pfd[0]);
    usleep(200000);
    kill(pid, SIGUSR1);
    check(waitpid(pid, &status, 0) == pid && WIFEXITED(status) && WEXITSTATUS(status) == 0,
          "flock: a caught signal ends the wait (EINTR)");
    close(fd);

    /* Record locks: of the process, by byte range; flock's are apart. */
    fd = open(f, O_RDWR);
    check(flock(fd, LOCK_EX) == 0, "flock beside the record locks");
    fl = (struct flock){.l_type = F_WRLCK, .l_whence = SEEK_SET, .l_start = 0, .l_len = 10};
    check(fcntl(fd, F_SETLK, &fl) == 0, "F_SETLK 0..10 (an flock lock does not meet it)");
    check(in_child(try_bytes_0_9, f) == 1, "F_SETLK: the range is taken (EAGAIN)");
    check(in_child(try_read_0_9, f) == 1, "F_SETLK: a read lock too");
    check(in_child(try_bytes_10_19, f) == 0, "F_SETLK: the next bytes are free");
    check(in_child(get_holder, f) == 0, "F_GETLK reports the holder");
    fl = (struct flock){.l_type = F_WRLCK, .l_whence = SEEK_SET, .l_start = 0, .l_len = 0};
    check(fcntl(fd, F_GETLK, &fl) == 0 && fl.l_type == F_UNLCK, "F_GETLK: one's own locks never conflict");
    /* Unlocking the middle splits the lock. */
    fl = (struct flock){.l_type = F_UNLCK, .l_whence = SEEK_SET, .l_start = 2, .l_len = 3};
    check(fcntl(fd, F_SETLK, &fl) == 0, "F_SETLK F_UNLCK 2..5");
    check(in_child(try_bytes_0_9, f) == 1, "split: the ends are still locked");
    fl = (struct flock){.l_type = F_UNLCK, .l_whence = SEEK_SET, .l_start = 0, .l_len = 0};
    check(fcntl(fd, F_SETLK, &fl) == 0 && in_child(try_bytes_0_9, f) == 0, "F_UNLCK to the end lets go");
    /* To the end of the file, then gone when the process closes any fd on
     * the file (POSIX), even another one. */
    fl = (struct flock){.l_type = F_WRLCK, .l_whence = SEEK_SET, .l_start = 100, .l_len = 0};
    check(fcntl(fd, F_SETLK, &fl) == 0, "F_SETLK 100..");
    check(in_child(try_bytes_100_on, f) == 1, "F_SETLK: locked to the end");
    fd2 = open(f, O_RDONLY);
    close(fd2);
    check(in_child(try_bytes_100_on, f) == 0, "record locks go when the process closes an fd on the file");
    check(in_child(try_flock_ex, f) == 1, "but the flock lock stays");
    /* lockf: a record lock from the position on. */
    check(lseek(fd, 0, SEEK_SET) == 0 && lockf(fd, F_TLOCK, 10) == 0, "lockf F_TLOCK");
    check(in_child(try_bytes_0_9, f) == 1, "lockf: locked");
    check(lockf(fd, F_ULOCK, 10) == 0 && in_child(try_bytes_0_9, f) == 0, "lockf F_ULOCK");
    close(fd);
    unlink(f);
}

/* fstat of what `path` names now (an fd's view, not the path's twice). */
static int stat_named(const char *path, struct stat *st) {
    int fd = open(path, O_RDONLY), ok;
    ok = fd >= 0 && fstat(fd, st) == 0;
    close(fd);
    return ok;
}

static void inodes(void) {
    const char *a = in_dir("inode-a"), *b = in_dir("inode-b");
    struct stat before, after;
    char buf[8] = {0};
    int fd;

    unlink(b);
    fd = open(a, O_RDWR | O_CREAT | O_TRUNC, 0644);
    check(fd >= 0 && write(fd, "one", 3) == 3, "open inode-a");
    check(fstat(fd, &before) == 0, "fstat inode-a");
    /* The fd follows the file through a rename. */
    check(rename(a, b) == 0, "rename");
    check(pwrite(fd, "two", 3, 3) == 3, "write after the rename");
    check(stat_named(b, &after) && after.st_size == 6, "the write reached the renamed file");
    check(after.st_ino == before.st_ino, "the inode number stays across a rename");
    /* A file renamed over keeps living for its fds. */
    close(open(a, O_WRONLY | O_CREAT | O_TRUNC, 0644));
    check(rename(a, b) == 0, "rename over");
    check(pread(fd, buf, 6, 0) == 6 && memcmp(buf, "onetwo", 6) == 0, "the replaced file is still read");
    check(fstat(fd, &after) == 0 && after.st_ino == before.st_ino, "fstat: still the same file");
    check(stat_named(b, &after) && after.st_ino != before.st_ino && after.st_size == 0, "the name is the new file");
    close(fd);
    unlink(b);
}

#define WHOLE 20000

/* One read gives a file's bytes up to the count or the end, on DIR's
 * filesystem and on the image's. */
static void whole_reads(void) {
    static char out[WHOLE], in[WHOLE];
    const char *f = in_dir("whole");
    struct stat st;
    int fd, i;

    for (i = 0; i < WHOLE; i++) {
        out[i] = (char)(i * 7 + i / 251);
    }
    fd = open(f, O_RDWR | O_CREAT | O_TRUNC, 0644);
    check(fd >= 0 && write(fd, out, WHOLE) == WHOLE, "write 20000 bytes");
    check(lseek(fd, 0, SEEK_SET) == 0, "lseek to 0");
    check(read(fd, in, WHOLE) == WHOLE && memcmp(in, out, WHOLE) == 0, "one read of 20000 bytes");
    check(read(fd, in, 100) == 0, "a read at the end");
    check(pread(fd, in, WHOLE, 15000) == WHOLE - 15000 && memcmp(in, out + 15000, WHOLE - 15000) == 0,
          "one pread up to the end");
    close(fd);
    unlink(f);
    fd = open("/bin/sh", O_RDONLY);
    check(fd >= 0 && fstat(fd, &st) == 0 && st.st_size > WHOLE, "open /bin/sh");
    check(read(fd, in, WHOLE) == WHOLE, "one read of 20000 bytes of /bin/sh");
    close(fd);
}

/* A rename while another process waits in a read of the console. */
static void console_reader(void) {
    const char *a = in_dir("tree-a"), *b = in_dir("tree-b");
    pid_t reader, renamer;
    int st = 0, i;

    close(open(a, O_WRONLY | O_CREAT | O_TRUNC, 0644));
    reader = fork();
    if (reader == 0) {
        char c;
        int fd = open("/dev/console/data", O_RDONLY);
        _exit(fd >= 0 && read(fd, &c, 1) >= 0 ? 0 : 1);
    }
    usleep(300000);
    renamer = fork();
    if (renamer == 0) {
        _exit(rename(a, b) == 0 ? 0 : 1);
    }
    for (i = 0; i < 50 && waitpid(renamer, &st, WNOHANG) == 0; i++) {
        usleep(100000);
    }
    check(i < 50, "a rename waited for a process reading the console");
    kill(reader, SIGKILL);
    waitpid(reader, NULL, 0);
    if (i == 50) {
        waitpid(renamer, &st, 0);
    }
    check(WIFEXITED(st) && WEXITSTATUS(st) == 0, "rename next to a console reader");
    unlink(a);
    unlink(b);
}

int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "child") == 0) {
        return child(argc, argv);
    }
    snprintf(dir, sizeof dir, "%s", argc > 1 ? argv[1] : "/tmp");
    excl();
    positional();
    cloexec("/bin/etc/fileio_smoke");
    open_flags();
    locks();
    inodes();
    whole_reads();
    if (argc == 1) {
        console_reader();
    }
    return failures != 0;
}
