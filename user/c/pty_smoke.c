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
 */
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
extern void *memmem(const void *, size_t, const void *, size_t);
#include <sys/types.h>
#include <sys/wait.h>
#include <termios.h>
#include <unistd.h>

#include <pty.h>

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

int main(int argc, char **argv) {
    int stage = (argc > 1) ? atoi(argv[1]) : 0;
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
        int dm = open("/dev/ptmx", O_RDWR | O_NOCTTY);
        printf("[ FAIL ] pty openpty (%s); open=%d", strerror(errno), dm);
        if (dm >= 0) {
            unsigned int n = 0;
            int rc = ioctl(dm, TIOCGPTN, &n);
            printf(" TIOCGPTN=%d errno=%s", rc, strerror(errno));
            if (rc == 0) {
                printf(" n=%u", n);
                char sp[32];
                snprintf(sp, sizeof(sp), "/dev/pts/%u", n);
                int ds = open(sp, O_RDWR | O_NOCTTY);
                printf(" slave=%d errno=%s", ds, strerror(errno));
                if (ds >= 0) close(ds);
            }
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
