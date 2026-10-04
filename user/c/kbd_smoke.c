/* kbd-smoke: boot-CI guest test for /dev/console/kbd (the console module's
 * kbdev.rs, docs/tty.md).
 *
 * The file is held by one program at a time: a second open fails, and the
 * first fd's close, or its holder's exit, lets the next one in. With the
 * file open, nothing is readable until the host types Shift+A through the
 * QEMU monitor (test.sh asks for it when this prints "ready"); then the
 * events are "d 42", "d 30" (with " A" when a keymap is loaded), "u 30" and
 * "u 42", the first through a read that waits, the rest through poll and
 * read. Prints [ OK ] kbd.
 */
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdio.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

#define KBD "/dev/console/kbd"

static int fail(const char *what) {
    printf("[ FAIL ] kbd %s (errno %d)\n", what, errno);
    return 1;
}

/* Whether `line` is the event `want` ("d 30"), with the key's character
 * allowed after it when `ch` is set. */
static int event_is(const char *line, const char *want, const char *ch) {
    size_t n = strlen(want);
    if (strncmp(line, want, n) != 0) {
        return 0;
    }
    return line[n] == '\0' || (ch && line[n] == ' ' && strcmp(line + n + 1, ch) == 0);
}

int main(void) {
    int fd = open(KBD, O_RDONLY);
    if (fd < 0) {
        return fail("open");
    }
    if (open(KBD, O_RDONLY) >= 0) {
        return fail("second open did not fail");
    }
    struct pollfd p = { .fd = fd, .events = POLLIN };
    if (poll(&p, 1, 0) != 0) {
        return fail("readable before any key");
    }

    printf("ready\n");
    fflush(stdout);

    /* Up to 4 lines: the first read waits for the host's keys (test.sh
     * bounds the wait). */
    char buf[256];
    size_t got = 0;
    int lines = 0;
    ssize_t n = read(fd, buf, sizeof buf - 1);
    if (n <= 0) {
        return fail("waiting read");
    }
    got = (size_t)n;
    for (;;) {
        lines = 0;
        for (size_t i = 0; i < got; i++) {
            lines += buf[i] == '\n';
        }
        if (lines >= 4) {
            break;
        }
        p.revents = 0;
        if (poll(&p, 1, 20000) != 1 || !(p.revents & POLLIN)) {
            return fail("poll for the next event");
        }
        n = read(fd, buf + got, sizeof buf - 1 - got);
        if (n <= 0) {
            return fail("read after poll");
        }
        got += (size_t)n;
    }
    buf[got] = '\0';

    const char *want[] = { "d 42", "d 30", "u 30", "u 42" };
    char *save = NULL;
    char *line = strtok_r(buf, "\n", &save);
    for (int i = 0; i < 4; i++, line = strtok_r(NULL, "\n", &save)) {
        if (!line || !event_is(line, want[i], i == 1 ? "A" : NULL)) {
            printf("[ FAIL ] kbd event %d: got \"%s\", want \"%s\"\n", i, line ? line : "", want[i]);
            return 1;
        }
    }

    close(fd);
    fd = open(KBD, O_RDONLY);
    if (fd < 0) {
        return fail("open after close");
    }
    close(fd);
    pid_t pid = fork();
    if (pid == 0) {
        /* Held at exit, never closed. */
        _exit(open(KBD, O_RDONLY) < 0);
    }
    int status;
    if (pid < 0 || waitpid(pid, &status, 0) != pid || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        return fail("child open");
    }
    fd = open(KBD, O_RDONLY);
    if (fd < 0) {
        return fail("open after the holder's exit");
    }
    close(fd);
    printf("[ OK ] kbd\n");
    return 0;
}
