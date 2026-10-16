/* mouse-smoke: boot-CI guest test for /dev/console/mouse (the console
 * module's mouse.rs fed by the usb_hid module, docs/tty.md, docs/usb.md).
 *
 * The file is held by one program at a time: a second open fails, and the
 * first fd's close lets the next one in. With the file open, nothing is
 * readable until the host moves the launcher's usb-mouse through the QEMU
 * monitor (test.sh asks for it when this prints "ready": 10 right and 5
 * up, then a left click); then the events are motion ("m DX DY" lines,
 * which add up to 10 -5), "d 272" and "u 272" (BTN_LEFT), the first
 * through a read that waits, the rest through poll and read. Prints
 * [ OK ] mouse.
 */
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#define MOUSE "/dev/console/mouse"

static int fail(const char *what) {
    printf("[ FAIL ] mouse %s (errno %d)\n", what, errno);
    return 1;
}

int main(void) {
    int fd = open(MOUSE, O_RDONLY);
    if (fd < 0) {
        return fail("open");
    }
    if (open(MOUSE, O_RDONLY) >= 0) {
        return fail("second open did not fail");
    }
    struct pollfd p = { .fd = fd, .events = POLLIN };
    if (poll(&p, 1, 0) != 0) {
        return fail("readable before the mouse moved");
    }

    printf("ready\n");
    fflush(stdout);

    /* Up to the release: the first read waits for the host (test.sh
     * bounds the wait). */
    char buf[1024];
    size_t got = 0;
    ssize_t n = read(fd, buf, sizeof buf - 1);
    if (n <= 0) {
        return fail("waiting read");
    }
    got = (size_t)n;
    for (;;) {
        buf[got] = '\0';
        if (strstr(buf, "u 272\n")) {
            break;
        }
        p.revents = 0;
        if (got == sizeof buf - 1 || poll(&p, 1, 20000) != 1 || !(p.revents & POLLIN)) {
            printf("got: %s", buf);
            return fail("poll for the next event");
        }
        n = read(fd, buf + got, sizeof buf - 1 - got);
        if (n <= 0) {
            return fail("read after poll");
        }
        got += (size_t)n;
    }

    /* Motion first, then the press, then the release. */
    int dx = 0, dy = 0, step = 0;
    char *save = NULL;
    for (char *line = strtok_r(buf, "\n", &save); line; line = strtok_r(NULL, "\n", &save)) {
        int x, y;
        if (step == 0 && sscanf(line, "m %d %d", &x, &y) == 2) {
            dx += x;
            dy += y;
        } else if (step == 0 && strcmp(line, "d 272") == 0) {
            step = 1;
        } else if (step == 1 && strcmp(line, "u 272") == 0) {
            step = 2;
        } else {
            printf("[ FAIL ] mouse unexpected event \"%s\"\n", line);
            return 1;
        }
    }
    if (step != 2 || dx != 10 || dy != -5) {
        printf("[ FAIL ] mouse motion %d %d (want 10 -5), click %d\n", dx, dy, step);
        return 1;
    }

    close(fd);
    fd = open(MOUSE, O_RDONLY);
    if (fd < 0) {
        return fail("open after close");
    }
    close(fd);
    printf("[ OK ] mouse\n");
    return 0;
}
