/* pty-smoke: boot-CI guest test for the myos pty implementation.
 *
 * Allocates a pty pair (openpty), forks a child running `cat` on the pty,
 * and verifies the full round trip through the slave line discipline:
 *   1. master write "ptyline\n"      -> cooked discipline: echo back to
 *      master ("ptyline\r\n" with OPOST/ONLCR) AND committed line to slave.
 *   2. cat echoes the line back      -> second "ptyline\r\n" on the master.
 *   3. child exit closes last slave fd -> master read returns EIO.
 *
 * PASS = both lines read on the master, then EIO. Exit code 0/1.
 *
 * `pty_smoke 4`: more pairs at once than the kernel once allowed (it had
 * four), each with its own two-digit-capable name and inode numbers and a
 * line through it, and a closed pair's index used again.
 *
 * `pty_smoke 6`: a session's claim on a pair ends with its leader. A child
 * makes a session, claims the slave (TIOCSCTTY) and exits; the next session
 * leader to get its pid (pids are task slots, reused at once) does not hold
 * the pair, and a third session's claim goes through: /proc/self/tty names
 * the pair (issue #448: a getty respawned on ttyS0 while a daemon had taken
 * the dead one's pid logged in without a controlling terminal).
 *
 * `pty_smoke 7 USER PASSWORD`: a pair is its user's (docs/security.md).
 * The caller's at first, who opened clone for it: a child that enters USER
 * does not open it. A child that makes it its session's terminal and then
 * enters USER (as login does) hands it over: USER opens its data, the
 * caller no longer opens its data or ctl. Once that session ends it is the
 * caller's again.
 *
 * `pty_smoke 5`: writers faster than their reader. Children each write
 * eight times what a pair's output ring holds to their slave while the
 * parent drains the masters one after the other: a writer waits for room,
 * also when it shares a CPU with the reader (the kernel once spun there with
 * interrupts off, and the reader on that CPU never ran).
 */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
extern void *memmem(const void *, size_t, const void *, size_t);
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <termios.h>
#include <unistd.h>

#include <pty.h>
#include <sys/myos_extra.h>

static int read_until(int fd, const char *needle, int max_chunks) {
    char buf[256];
    size_t nl = strlen(needle);
    size_t have = 0;
    char acc[1024];
    for (int i = 0; i < max_chunks; i++) {
        ssize_t n = read(fd, buf, sizeof(buf));
        if (n < 0) return -1;
        for (ssize_t k = 0; k < n; k++) {
            if (have < sizeof(acc)) acc[have++] = buf[k];
        }
        for (size_t p = 0; p + nl <= have; p++) {
            if (memcmp(acc + p, needle, nl) == 0) return 0;
        }
    }
    return -1;
}

#define MANY 12

/* Stage 4: MANY pairs open at once. */
static int many(void) {
    int m[MANY], sl[MANY];
    char names[MANY][32];
    struct stat st[MANY];
    for (int i = 0; i < MANY; i++) {
        if (openpty(&m[i], &sl[i], names[i], NULL, NULL) != 0) {
            printf("[ FAIL ] ptys openpty %d of %d (%s)\n", i + 1, MANY, strerror(errno));
            return 1;
        }
        if (stat(names[i], &st[i]) != 0) {
            printf("[ FAIL ] ptys stat %s (%s)\n", names[i], strerror(errno));
            return 1;
        }
        for (int j = 0; j < i; j++) {
            if (strcmp(names[i], names[j]) == 0 || st[i].st_ino == st[j].st_ino) {
                printf("[ FAIL ] ptys %s and %s share a name or an inode\n", names[i], names[j]);
                return 1;
            }
        }
    }
    for (int i = 0; i < MANY; i++) {
        char rb[8];
        if (write(m[i], "x\n", 2) != 2 || read(sl[i], rb, sizeof rb) != 2 || rb[0] != 'x') {
            printf("[ FAIL ] ptys line through %s\n", names[i]);
            return 1;
        }
    }
    /* The third pair's index is free again once both its ends close. */
    close(m[2]);
    close(sl[2]);
    char again[32];
    int am = -1, as = -1;
    if (openpty(&am, &as, again, NULL, NULL) != 0 || strcmp(again, names[2]) != 0) {
        printf("[ FAIL ] ptys %s not reused (got %s)\n", names[2], am < 0 ? "none" : again);
        return 1;
    }
    close(am);
    close(as);
    for (int i = 0; i < MANY; i++) {
        if (i != 2) {
            close(m[i]);
            close(sl[i]);
        }
    }
    printf("[ OK ] ptys %d pairs\n", MANY);
    return 0;
}

#define FLOOD_PAIRS 5
#define FLOOD_BYTES (32 * 1024)

static int flood(void) {
    int m[FLOOD_PAIRS];
    pid_t pid[FLOOD_PAIRS];
    for (int i = 0; i < FLOOD_PAIRS; i++) {
        int sl;
        if (openpty(&m[i], &sl, NULL, NULL, NULL) != 0) {
            printf("[ FAIL ] flood openpty (%s)\n", strerror(errno));
            return 1;
        }
        pid[i] = fork();
        if (pid[i] < 0) {
            printf("[ FAIL ] flood fork (%s)\n", strerror(errno));
            return 1;
        }
        if (pid[i] == 0) {
            char chunk[4096];
            memset(chunk, 'a' + i, sizeof chunk);
            for (int k = 0; k < i; k++) {
                close(m[k]);
            }
            close(m[i]);
            for (int sent = 0; sent < FLOOD_BYTES; sent += (int)sizeof chunk) {
                if (write(sl, chunk, sizeof chunk) != (ssize_t)sizeof chunk) {
                    _exit(1);
                }
            }
            _exit(0);
        }
        close(sl);
    }
    for (int i = 0; i < FLOOD_PAIRS; i++) {
        char buf[4096];
        long got = 0;
        for (;;) {
            ssize_t n = read(m[i], buf, sizeof buf);
            if (n <= 0) {
                break; /* EIO: the child closed its slave */
            }
            for (ssize_t k = 0; k < n; k++) {
                if (buf[k] != 'a' + i) {
                    printf("[ FAIL ] flood pair %d: a byte from elsewhere\n", i);
                    return 1;
                }
            }
            got += n;
        }
        int st = 0;
        waitpid(pid[i], &st, 0);
        if (got != FLOOD_BYTES || !WIFEXITED(st) || WEXITSTATUS(st) != 0) {
            printf("[ FAIL ] flood pair %d: %ld bytes, status %d\n", i, got, st);
            return 1;
        }
        close(m[i]);
    }
    return 0;
}

/* A session leader that claims the slave `s` and exits with `code`. */
static pid_t claimant(int s, int code) {
    pid_t pid = fork();
    if (pid == 0) {
        setsid();
        if (ioctl(s, TIOCSCTTY, 0) != 0) {
            _exit(2);
        }
        char tty[64];
        ssize_t n = readlink("/proc/self/tty", tty, sizeof tty - 1);
        _exit(n > 0 && strncmp(tty, "/dev/pts/", 9) == 0 ? code : 3);
    }
    return pid;
}

static int reclaim(void) {
    int m = -1, s = -1;
    if (openpty(&m, &s, NULL, NULL, NULL) != 0) {
        printf("[ FAIL ] s6 openpty (%s)\n", strerror(errno));
        return 1;
    }
    int status = 0;
    pid_t first = claimant(s, 0);
    if (first < 0 || waitpid(first, &status, 0) != first || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        printf("[ FAIL ] s6 first claim (status %d)\n", status);
        return 1;
    }
    /* Session leaders kept alive until one has the first claimant's pid
     * (the first fork after the reap usually does); each says when it leads
     * its session. */
    pid_t kept[16];
    int nkept = 0, reused = 0;
    int hold[2]; /* the leaders wait on its read end until it is closed */
    if (pipe(hold) != 0) {
        printf("[ FAIL ] s6 pipe (%s)\n", strerror(errno));
        return 1;
    }
    while (nkept < 16 && !reused) {
        int fds[2];
        if (pipe(fds) != 0) {
            break;
        }
        pid_t pid = fork();
        if (pid == 0) {
            close(fds[0]);
            close(hold[1]);
            setsid();
            if (write(fds[1], "s", 1) != 1) {
                _exit(1);
            }
            char c;
            (void)read(hold[0], &c, 1);
            _exit(0);
        }
        close(fds[1]);
        char c;
        if (pid < 0 || read(fds[0], &c, 1) != 1) {
            close(fds[0]);
            break;
        }
        close(fds[0]);
        kept[nkept++] = pid;
        reused = pid == first;
    }
    if (!reused) {
        printf("[ INFO ] s6 pid %d not reused by %d leaders\n", (int)first, nkept);
    }
    pid_t third = claimant(s, 0);
    int ok = third > 0 && waitpid(third, &status, 0) == third && WIFEXITED(status) && WEXITSTATUS(status) == 0;
    if (!ok) {
        printf("[ FAIL ] s6 claim after the leader's pid was reused (status %d)\n", status);
    }
    close(hold[1]);
    close(hold[0]);
    for (int i = 0; i < nkept; i++) {
        waitpid(kept[i], NULL, 0);
    }
    close(s);
    close(m);
    return ok ? 0 : 1;
}

/* Whether the current process may open `path` for reading and writing. */
static int opens(const char *path) {
    int fd = open(path, O_RDWR | O_NOCTTY);
    if (fd < 0) {
        return 0;
    }
    close(fd);
    return 1;
}

static int handover(const char *user, const char *password) {
    int m = -1, s = -1;
    char name[32], ctl[48];
    if (openpty(&m, &s, name, NULL, NULL) != 0) {
        printf("[ FAIL ] s7 openpty (%s)\n", strerror(errno));
        return 1;
    }
    snprintf(ctl, sizeof ctl, "%.*s/ctl", (int)(strlen(name) - strlen("/data")), name);
    if (!opens(name) || !opens(ctl)) {
        printf("[ FAIL ] s7 the opener cannot open %s\n", name);
        return 1;
    }
    int status = 0;
    pid_t other = fork();
    if (other == 0) {
        _exit(myos_setuser(user, password) != 0 ? 2 : opens(name) ? 3 : 0);
    }
    if (other < 0 || waitpid(other, &status, 0) != other || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        printf("[ FAIL ] s7 %s and the opener's terminal (status %d)\n", user, status);
        return 1;
    }
    int entered[2], done[2];
    if (pipe(entered) != 0 || pipe(done) != 0) {
        printf("[ FAIL ] s7 pipe (%s)\n", strerror(errno));
        return 1;
    }
    pid_t pid = fork();
    if (pid == 0) {
        close(entered[0]);
        close(done[1]);
        setsid();
        if (ioctl(s, TIOCSCTTY, 0) != 0 || myos_setuser(user, password) != 0) {
            _exit(2);
        }
        char c = opens(name) ? 'y' : 'n';
        if (write(entered[1], &c, 1) != 1) {
            _exit(1);
        }
        (void)read(done[0], &c, 1);
        _exit(0);
    }
    close(entered[1]);
    close(done[0]);
    char c = 0;
    int ok = 1;
    if (pid < 0 || read(entered[0], &c, 1) != 1 || c != 'y') {
        printf("[ FAIL ] s7 %s did not get the session's terminal (%c)\n", user, c ? c : '-');
        ok = 0;
    } else if (opens(name) || opens(ctl)) {
        printf("[ FAIL ] s7 LEAK: %s's terminal %s still opens for its opener\n", user, name);
        ok = 0;
    }
    close(done[1]);
    if (pid > 0) {
        waitpid(pid, &status, 0);
    }
    if (ok && !opens(name)) {
        printf("[ FAIL ] s7 %s is not the opener's again after the session\n", name);
        ok = 0;
    }
    close(entered[0]);
    close(s);
    close(m);
    return ok ? 0 : 1;
}

int main(int argc, char **argv) {
    int stage = (argc > 1) ? atoi(argv[1]) : 0;
    if (stage == 7 && argc > 3) {
        return handover(argv[2], argv[3]);
    }
    if (stage == 6) {
        return reclaim();
    }
    if (stage == 5) {
        return flood();
    }
    if (stage == 4) {
        return many();
    }
    if (stage >= 3) {
        /* Stage 3: forkpty child writes without exec'ing cat. */
        int m = -1; char nm[32];
        if (openpty(&m, NULL, nm, NULL, NULL) != 0) {
            printf("[ FAIL ] s3 openpty (%s)\n", strerror(errno));
            return 1;
        }
        pid_t pid = forkpty(&m, NULL, NULL, NULL);
        if (pid < 0) {
            printf("[ FAIL ] s3 forkpty (%s)\n", strerror(errno));
            return 1;
        }
        if (pid == 0) {
            ssize_t wn = write(1, "child\n", 6);
            printf("[dbg] child wn=%ld errno=%d\n", wn, errno);
            _exit(0);
        }
        char mb[64];
        ssize_t mn = read(m, mb, sizeof(mb));
        int re = errno;
        printf("[ INFO ] s3 master read n=%ld errno=%d", mn, re);
        if (mn > 0) printf(" (%.*s)", (int)mn, mb);
        printf("\n");
        int st = 0;
        waitpid(pid, &st, 0);
        printf("[ INFO ] s3 wait status=%d\n", st);
        close(m);
        return 0;
    }
    if (stage >= 2) {
        /* Stage 2: kernel round-trip without fork/exec/cat. */
        int mm = -1, ss = -1; char nm[32];
        if (openpty(&mm, &ss, nm, NULL, NULL) != 0) {
            printf("[ FAIL ] s2 openpty (%s)\n", strerror(errno));
            return 1;
        }
        const char *in = "hi\n";
        if (write(mm, in, 3) != 3) {
            printf("[ FAIL ] s2 master write (%s)\n", strerror(errno));
            return 1;
        }
        char rb[64];
        ssize_t rn = read(ss, rb, sizeof(rb));
        if (rn != 3 || memcmp(rb, "hi\n", 3) != 0) {
            printf("[ FAIL ] s2 slave read rn=%ld\n", rn);
            return 1;
        }
        const char *out = "out\n";
        if (write(ss, out, 4) != 4) {
            printf("[ FAIL ] s2 slave write (%s)\n", strerror(errno));
            return 1;
        }
        char mb[64];
        ssize_t mn = read(mm, mb, sizeof(mb));
        if (mn < 0 || !memmem(mb, mn, "out\r\n", 5)) {
            printf("[ FAIL ] s2 master read mn=%ld\n", mn);
            return 1;
        }
        printf("[ INFO ] s2 roundtrip ok, out=%.*s", (int)mn, mb);
        close(ss);
        errno = 0;
        rn = read(mm, mb, sizeof(mb));
        if (rn >= 0) {
            printf("[ FAIL ] s2 no EIO after slave close (rn=%ld)\n", rn);
            return 1;
        }
        printf("[ OK ] s2 EIO\n");
        close(mm);
        return 0;
    }
    if (stage >= 1) {
        int mm = -1; char nm[32];
        if (openpty(&mm, NULL, nm, NULL, NULL) != 0) {
            printf("[ FAIL ] s1 openpty (%s)\n", strerror(errno));
            return 1;
        }
        printf("[ INFO ] s1 slave %s\n", nm);
        close(mm);
        return 0;
    }
    int m = -1;
    char name[32];
    if (openpty(&m, NULL, name, NULL, NULL) != 0) {
        /* Reproduce step-by-step so the failure names the exact call. */
        int dm = open("/dev/pts/clone", O_RDWR | O_NOCTTY);
        printf("[ FAIL ] pty openpty (%s); clone=%d", strerror(errno), dm);
        if (dm >= 0) {
            char link[32], target[64];
            snprintf(link, sizeof link, "/proc/self/fd/%d", dm);
            ssize_t n = readlink(link, target, sizeof target - 1);
            printf(" link=%.*s errno=%s", n < 0 ? 0 : (int)n, target, strerror(errno));
            close(dm);
        }
        printf("\n");
        return 1;
    }
    printf("[ INFO ] pty slave at %s\n", name);

    pid_t pid = forkpty(&m, NULL, NULL, NULL);
    if (pid < 0) {
        printf("[ FAIL ] pty forkpty (%s)\n", strerror(errno));
        close(m);
        return 1;
    }
    if (pid == 0) {
        /* Child: pty slave is 0/1/2 and ctty; run cat (echo by kernel
         * discipline is verified by the parent reading the master). */
        execlp("/bin/custom/cat", "cat", (char *)0);
        _exit(127);
    }

    /* Parent: feed one line; expect (a) the kernel discipline echo and
     * (b) cat's own echo of the committed line. */
    const char *line = "ptyline\n";
    if (write(m, line, strlen(line)) != (ssize_t)strlen(line)) {
        printf("[ FAIL ] pty master write (%s)\n", strerror(errno));
        goto fail;
    }
    if (read_until(m, "ptyline\r\n", 64) != 0 ||
        read_until(m, "ptyline\r\n", 64) != 0) {
        printf("[ FAIL ] pty echo/roundtrip\n");
        goto fail;
    }

    /* EOF (^D, VEOF) so cat's stdin read returns 0 and the child exits. */
    const char eofc = 0x04;
    if (write(m, &eofc, 1) != 1) {
        printf("[ FAIL ] pty eof write (%s)\n", strerror(errno));
        goto fail;
    }
    int st = 0;
    waitpid(pid, &st, 0);
    if (!WIFEXITED(st) || WEXITSTATUS(st) != 0) {
        printf("[ FAIL ] pty child exit %d\n", st);
        goto fail;
    }

    /* Child exited -> last slave fd closed -> EIO expected. */
    char buf[64];
    errno = 0;
    ssize_t n = read(m, buf, sizeof(buf));
    if (!(n < 0 && errno == EIO)) {
        printf("[ FAIL ] pty EIO after close (n=%ld errno=%d)\n", (long)n, (int)errno);
        goto fail;
    }
    close(m);
    printf("[ OK ] pty\n");
    return 0;

fail:
    close(m);
    return 1;
}
