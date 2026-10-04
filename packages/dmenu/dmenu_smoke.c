/* dmenu-smoke: the dmenu boot test's eyes on the framebuffer.
 *
 *   dmenu_smoke server  wait (up to 60 s) for the X server's socket, so
 *                       that dmenu (which does not retry) finds it
 *   dmenu_smoke bar     wait for dmenu's bar: the selected item's #005577
 *                       somewhere along the screen's top
 *
 * Prints "[ OK ] dmenu <mode>", or what it last saw.
 */
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

#define SEL_BG 0x005577u
/* A row inside the bar (its height is the font's, about 16 pixels). */
#define Y 4
#define MAX_W 4096

/* Whether the server listens on display :0's socket yet. */
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
    static uint32_t row[MAX_W];
    char line[128];
    unsigned w, h, depth, pitch;
    int ctl, fd;
    ssize_t n;

    if (argc != 2 || (strcmp(argv[1], "server") && strcmp(argv[1], "bar"))) {
        fprintf(stderr, "usage: dmenu_smoke server|bar\n");
        return 2;
    }
    if (!strcmp(argv[1], "server")) {
        for (int i = 0; i < 60; i++) {
            if (server_up()) {
                printf("[ OK ] dmenu server\n");
                return 0;
            }
            sleep(1);
        }
        printf("[ FAIL ] dmenu server: no socket\n");
        return 1;
    }
    ctl = open("/dev/fb/ctl", O_RDONLY);
    n = ctl < 0 ? -1 : read(ctl, line, sizeof line - 1);
    if (n <= 0) {
        printf("[ FAIL ] dmenu reading /dev/fb/ctl\n");
        return 1;
    }
    line[n] = '\0';
    close(ctl);
    if (sscanf(line, "%u %u %u %*s %u", &w, &h, &depth, &pitch) != 4 || depth != 32 || w > MAX_W
        || h <= Y) {
        printf("[ FAIL ] dmenu /dev/fb/ctl is not a 32-bit screen: %s", line);
        return 1;
    }
    fd = open("/dev/fb/data", O_RDONLY);
    if (fd < 0) {
        printf("[ FAIL ] dmenu opening /dev/fb/data\n");
        return 1;
    }
    for (int i = 0; i < 60; i++) {
        if (lseek(fd, (off_t)Y * pitch, SEEK_SET) < 0 || read(fd, row, w * 4) != (ssize_t)(w * 4)) {
            printf("[ FAIL ] dmenu reading /dev/fb/data\n");
            return 1;
        }
        for (unsigned x = 0; x < w; x++) {
            if ((row[x] & 0xffffff) == SEL_BG) {
                printf("[ OK ] dmenu bar\n");
                return 0;
            }
        }
        sleep(1);
    }
    printf("[ FAIL ] dmenu bar: no %06x in row %d (its left pixel is %06x)\n", SEL_BG, Y,
           (unsigned)(row[0] & 0xffffff));
    return 1;
}
