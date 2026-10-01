/*
 * Boot smoke for the optional Linux compatibility layer. Built as a static-PIE
 * x86_64 Linux binary against musl (not myos newlib) and run as
 * `linux linux-smoke`. Prints "LINUX-SMOKE OK" when every check passes.
 */
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/utsname.h>
#include <sys/wait.h>
#include <unistd.h>

static int failures;

static void check(int ok, const char *what) {
    printf("%s: %s\n", ok ? "ok" : "FAIL", what);
    if (!ok) {
        failures++;
    }
}

int main(int argc, char **argv) {
    if (argc > 1 && strcmp(argv[1], "child") == 0) {
        return 5;
    }
    setvbuf(stdout, NULL, _IONBF, 0);

    struct utsname u;
    check(uname(&u) == 0 && strcmp(u.sysname, "Linux") == 0, "uname");

    char cwd[256];
    check(getcwd(cwd, sizeof cwd) != NULL && cwd[0] == '/', "getcwd");

    /* stdio file write, raw read back, fstat. */
    const char *path = "/tmp/linux-smoke.txt";
    FILE *f = fopen(path, "w");
    check(f != NULL && fprintf(f, "hello linux\n") == 12 && fclose(f) == 0, "fopen/fprintf");
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

    check(unlink(path) == 0 && stat(path, &st) == -1 && errno == ENOENT, "unlink/ENOENT");

    if (failures) {
        printf("LINUX-SMOKE FAIL (%d)\n", failures);
        return 1;
    }
    printf("LINUX-SMOKE OK\n");
    return 0;
}
