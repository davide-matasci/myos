/* tap-smoke: boot-CI guest test for /dev/tap (modules/tap), the virtual
 * NICs whose wire is a program.
 *
 * Holds a free NIC's `prog` (a second open is refused), sets its address
 * and brings its link up through `ctl`, which then says so; a frame the
 * program writes comes out of `data` whole, one netd sends through `data`
 * reaches `prog` (its `poll` saying so), and nothing netd sends while the
 * link is down does; `data` reads 0 bytes with nothing waiting. Closing
 * `prog` takes the NIC away. Prints [ OK ] tap.
 */
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

static int fail(const char *what) {
    printf("[ FAIL ] tap %s (errno %d)\n", what, errno);
    return 1;
}

/* Whether `fd` has something to read within `ms`. */
static int readable(int fd, int ms) {
    struct pollfd p = {.fd = fd, .events = POLLIN};
    return poll(&p, 1, ms) == 1 && (p.revents & POLLIN);
}

int main(void) {
    char path[64];
    int n, prog = -1;
    for (n = 0; n < 4 && prog < 0; n++) {
        snprintf(path, sizeof path, "/dev/tap/%d/prog", n);
        prog = open(path, O_RDWR);
    }
    if (prog < 0)
        return fail("no free NIC");
    n--;
    if (open(path, O_RDWR) >= 0)
        return fail("a second open of prog was not refused");

    snprintf(path, sizeof path, "/dev/tap/%d/ctl", n);
    int ctl = open(path, O_RDWR);
    const char *mac = "mac 02:00:00:aa:bb:cc";
    if (ctl < 0 || write(ctl, mac, strlen(mac)) != (ssize_t)strlen(mac))
        return fail("set the address");
    snprintf(path, sizeof path, "/dev/tap/%d/data", n);
    int data = open(path, O_RDWR);
    if (data < 0)
        return fail("open data");

    /* Down: what netd sends goes nowhere. */
    unsigned char frame[60], got[2048];
    memset(frame, 0x5a, sizeof frame);
    if (write(data, frame, sizeof frame) != sizeof frame)
        return fail("send while down");
    if (readable(prog, 50))
        return fail("a frame through a link that is down");

    if (write(ctl, "up", 2) != 2)
        return fail("link up");
    char text[128] = {0};
    if (pread(ctl, text, sizeof text - 1, 0) <= 0 ||
        !strstr(text, "mac 02:00:00:aa:bb:cc\n") || !strstr(text, "link up\n"))
        return fail("ctl after up");

    /* The program to netd, whole. */
    if (read(data, got, sizeof got) != 0)
        return fail("data read with nothing waiting");
    frame[0] = 1;
    if (write(prog, frame, sizeof frame) != sizeof frame)
        return fail("program write");
    if (!readable(data, 1000) || read(data, got, sizeof got) != sizeof frame || got[0] != 1)
        return fail("frame from the program");

    /* netd to the program. */
    frame[0] = 2;
    if (write(data, frame, sizeof frame) != sizeof frame)
        return fail("send while up");
    if (!readable(prog, 1000) || read(prog, got, sizeof got) != sizeof frame || got[0] != 2)
        return fail("frame from netd");

    close(prog);
    if (open(path, O_RDWR) >= 0)
        return fail("the NIC outlived its program");
    printf("[ OK ] tap\n");
    return 0;
}
