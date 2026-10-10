/* btm-smoke: bottom's boot test, driving btm on a pty as a person at a
 * terminal would (packages/bottom/test.sh).
 *
 * btm runs on the slave of a fresh 60x120 pty (forkpty), tall enough for
 * its process table to hold every process of a boot (with 40 rows it has
 * 8, and init, at 0% and a high pid, fell below them). Its screen must
 * show the CPU, memory and process widgets with what the kernel reports:
 * the widget titles, the RAM legend, and processes by name (init, and btm
 * itself) in the process table. Then `q` must end it, with status 0 and the
 * terminal given back.
 *
 * Prints `[ OK ] btm` and exits 0, or `[ FAIL ] btm <what>` with the tail of
 * the screen output, escaped.
 */
#include <dirent.h>
#include <errno.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#include <pty.h>

static char out[1 << 20];
static size_t out_len;

/* What the screen must show once btm has collected for a few seconds. */
static const char *const needles[] = {" CPU ", " Memory ", " Processes ", "RAM", "init", "btm"};
#define NEEDLES (sizeof needles / sizeof needles[0])

static int seen(const char *needle) {
    size_t n = strlen(needle);
    for (size_t i = 0; i + n <= out_len; i++) {
        if (memcmp(out + i, needle, n) == 0) {
            return 1;
        }
    }
    return 0;
}

/* Read what btm writes for up to `ms` milliseconds; -1 once the pty is
 * closed (btm exited). A cursor position query (DSR, `ESC [ 6 n`) gets the
 * answer a terminal gives: the top left. */
static int pump(int master, int ms) {
    struct pollfd p = {.fd = master, .events = POLLIN};
    int r = poll(&p, 1, ms);
    if (r <= 0) {
        return 0;
    }
    ssize_t n = read(master, out + out_len, sizeof out - 1 - out_len);
    if (n <= 0) {
        return -1;
    }
    for (ssize_t i = 0; i + 4 <= n; i++) {
        if (memcmp(out + out_len + i, "\x1b[6n", 4) == 0 && write(master, "\x1b[1;1R", 6) != 6) {
            return -1;
        }
    }
    out_len += (size_t)n;
    if (out_len > sizeof out - 4096) {
        /* Keep the latest screens. */
        memmove(out, out + out_len / 2, out_len - out_len / 2);
        out_len -= out_len / 2;
    }
    return 1;
}

static void dump_tail(void) {
    size_t from = out_len > 2000 ? out_len - 2000 : 0;
    for (size_t i = from; i < out_len; i++) {
        unsigned char c = (unsigned char)out[i];
        if (c == '\n' || (c >= 0x20 && c < 0x7f)) {
            putchar(c);
        } else {
            printf("\\x%02x", c);
        }
    }
    putchar('\n');
}

/* Each of btm's threads as /proc shows it: where a hang is. */
static void dump_threads(pid_t pid) {
    char path[64];
    snprintf(path, sizeof path, "/proc/%d/task", (int)pid);
    DIR *d = opendir(path);
    if (d == NULL) {
        return;
    }
    struct dirent *e;
    while ((e = readdir(d)) != NULL) {
        if (e->d_name[0] == '.') {
            continue;
        }
        char line[256];
        snprintf(path, sizeof path, "/proc/%d/task/%s/status", (int)pid, e->d_name);
        FILE *f = fopen(path, "r");
        if (f != NULL && fgets(line, sizeof line, f) != NULL) {
            printf("thread %s", line);
        }
        if (f != NULL) {
            fclose(f);
        }
    }
    closedir(d);
}

static int fail(const char *what, pid_t child) {
    printf("[ FAIL ] btm %s\n", what);
    dump_tail();
    if (child > 0) {
        dump_threads(child);
        kill(child, SIGKILL);
        waitpid(child, NULL, 0);
    }
    return 1;
}

int main(void) {
    struct winsize ws = {.ws_row = 60, .ws_col = 120};
    int master;
    pid_t child = forkpty(&master, NULL, NULL, &ws);
    if (child < 0) {
        printf("[ FAIL ] btm forkpty: %s\n", strerror(errno));
        return 1;
    }
    if (child == 0) {
        execlp("btm", "btm", (char *)NULL);
        printf("exec btm: %s\n", strerror(errno));
        _exit(127);
    }

    /* Under emulation the first screens take a while. */
    time_t start = time(NULL);
    size_t missing = NEEDLES;
    while (missing > 0 && time(NULL) - start < 90) {
        if (pump(master, 1000) < 0) {
            return fail("exited before showing its widgets", -1);
        }
        missing = 0;
        for (size_t i = 0; i < NEEDLES; i++) {
            missing += !seen(needles[i]);
        }
    }
    for (size_t i = 0; i < NEEDLES; i++) {
        if (!seen(needles[i])) {
            printf("not on the screen: \"%s\"\n", needles[i]);
        }
    }
    if (missing > 0) {
        return fail("screen incomplete", child);
    }

    if (write(master, "q", 1) != 1) {
        return fail("typing q", child);
    }
    start = time(NULL);
    int status = 0;
    pid_t done = 0;
    while (done == 0 && time(NULL) - start < 30) {
        pump(master, 500);
        done = waitpid(child, &status, WNOHANG);
    }
    if (done != child) {
        return fail("still running 30 s after q", child);
    }
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        printf("status 0x%x\n", status);
        return fail("did not exit cleanly", -1);
    }
    printf("[ OK ] btm\n");
    return 0;
}
