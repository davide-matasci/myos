/* dwm-smoke: the dwm boot test's eyes on the framebuffer.
 *
 *   dwm_smoke server wait (up to 60 s) for the X server's socket, so that
 *                    dwm (which does not retry) finds it
 *   dwm_smoke bar    wait for dwm's bar: the selected tag's box, #005577,
 *                    at the top left
 *   dwm_smoke gone   wait for it to be gone (dwm hid the bar)
 *
 * Prints "[ OK ] dwm <mode>", or what it last saw.
 */
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#define BAR_SEL 0x005577u
#define X 2
#define Y 2

/* Whether the server listens on display :0's socket yet (a plain connect:
 * XOpenDisplay would also try TCP each time). */
static int server_up(void) {
    struct sockaddr_un a = {.sun_family = AF_UNIX, .sun_path = "/tmp/.X11-unix/X0"};
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    int up = fd >= 0 && connect(fd, (struct sockaddr *)&a, sizeof a) == 0;
    if (fd >= 0) {
        close(fd);
    }
    return up;
}

int main(int argc, char **argv) {
    char line[128];
    unsigned w, h, depth, pitch;
    uint32_t p = 0;
    ssize_t n;
    int ctl, fd, want_bar;

    if (argc != 2 || (strcmp(argv[1], "server") && strcmp(argv[1], "bar") && strcmp(argv[1], "gone"))) {
        fprintf(stderr, "usage: dwm_smoke server|bar|gone\n");
        return 2;
    }
    if (!strcmp(argv[1], "server")) {
        for (int i = 0; i < 60; i++) {
            if (server_up()) {
                printf("[ OK ] dwm server\n");
                return 0;
            }
            sleep(1);
        }
        printf("[ FAIL ] dwm server: no socket\n");
        return 1;
    }
    want_bar = !strcmp(argv[1], "bar");
    ctl = open("/dev/fb/ctl", O_RDONLY);
    n = ctl < 0 ? -1 : read(ctl, line, sizeof line - 1);
    if (n <= 0) {
        printf("[ FAIL ] dwm reading /dev/fb/ctl\n");
        return 1;
    }
    line[n] = '\0';
    close(ctl);
    if (sscanf(line, "%u %u %u %*s %u", &w, &h, &depth, &pitch) != 4 || depth != 32) {
        printf("[ FAIL ] dwm /dev/fb/ctl is not a 32-bit screen: %s", line);
        return 1;
    }
    fd = open("/dev/fb/data", O_RDONLY);
    if (fd < 0) {
        printf("[ FAIL ] dwm opening /dev/fb/data\n");
        return 1;
    }
    for (int i = 0; i < 60; i++) {
        if (lseek(fd, (off_t)Y * pitch + X * 4, SEEK_SET) < 0 || read(fd, &p, 4) != 4) {
            printf("[ FAIL ] dwm reading /dev/fb/data\n");
            return 1;
        }
        if (((p & 0xffffff) == BAR_SEL) == want_bar) {
            printf("[ OK ] dwm %s\n", argv[1]);
            return 0;
        }
        sleep(1);
    }
    printf("[ FAIL ] dwm %s: pixel (%d, %d) is %06x\n", argv[1], X, Y, (unsigned)(p & 0xffffff));
    return 1;
}
