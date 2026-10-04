/* dwm-smoke: the dwm boot test's eyes on the framebuffer.
 *
 *   dwm_smoke server wait (up to 60 s) for the X server's socket, so that
 *                    dwm (which does not retry) finds it
 *   dwm_smoke bar    wait for dwm's bar: the selected tag's box, #005577,
 *                    at the top left
 *   dwm_smoke gone   wait for it to be gone (dwm hid the bar)
 *   dwm_smoke probe  whether the server answers a new connection's setup
 *                    within 10 s, the unix conversations' unread bytes and
 *                    the server's view: its top-level windows and the
 *                    root's pixel (when the bar does not come)
 *
 * Prints "[ OK ] dwm <mode>", or what it last saw.
 */
#include <fcntl.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>
#include <X11/Xlib.h>
#include <X11/Xutil.h>

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

/* Send an X connection setup (little endian, protocol 11.0, no auth) and
 * wait for the first byte of the server's answer (1: success). */
static int probe(void) {
    static const unsigned char setup[12] = {'l', 0, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0};
    struct sockaddr_un a = {.sun_family = AF_UNIX, .sun_path = "/tmp/.X11-unix/X0"};
    struct pollfd p;
    unsigned char b;
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);

    if (fd < 0 || connect(fd, (struct sockaddr *)&a, sizeof a) < 0) {
        printf("probe: no connection\n");
        return 1;
    }
    if (write(fd, setup, sizeof setup) != sizeof setup) {
        printf("probe: setup not sent\n");
        return 1;
    }
    p.fd = fd;
    p.events = POLLIN;
    if (poll(&p, 1, 10000) != 1 || read(fd, &b, 1) != 1) {
        printf("probe: the server does not answer\n");
        return 1;
    }
    printf("probe: the server answers (%d)\n", b);
    close(fd);
    return 0;
}

/* Each /net/unix conversation: its status, the bytes waiting in its data
 * and, for a listener, the connections waiting in its listen. */
static void conversations(void) {
    char path[48], status[32];
    struct stat st;

    for (int n = 0; n < 32; n++) {
        snprintf(path, sizeof path, "/net/unix/%d/status", n);
        int fd = open(path, O_RDONLY);
        if (fd < 0) {
            continue;
        }
        ssize_t len = read(fd, status, sizeof status - 1);
        close(fd);
        status[len > 0 ? len : 0] = '\0';
        printf("/net/unix/%d: %s", n, status);
        snprintf(path, sizeof path, "/net/unix/%d/data", n);
        if (stat(path, &st) == 0) {
            printf(" data=%lld", (long long)st.st_size);
        }
        snprintf(path, sizeof path, "/net/unix/%d/listen", n);
        if (stat(path, &st) == 0) {
            printf(" listen=%lld", (long long)st.st_size);
        }
        printf("\n");
    }
}

/* The server's view: the root's children (dwm's bar among them) and the
 * pixel at (X, Y) as the server has it. */
static void server_view(void) {
    Display *dpy = XOpenDisplay(":0");
    Window root, parent, *kids = NULL;
    unsigned nkids = 0;

    if (!dpy) {
        printf("view: no display\n");
        return;
    }
    root = DefaultRootWindow(dpy);
    if (XQueryTree(dpy, root, &root, &parent, &kids, &nkids)) {
        for (unsigned i = 0; i < nkids; i++) {
            XWindowAttributes wa;
            if (XGetWindowAttributes(dpy, kids[i], &wa)) {
                printf("view: window 0x%lx %dx%d+%d+%d %s%s\n", kids[i], wa.width, wa.height, wa.x, wa.y,
                       wa.map_state == IsViewable ? "viewable" : "unmapped",
                       wa.override_redirect ? " override-redirect" : "");
            }
        }
        XFree(kids);
    }
    /* Down the bar's left edge: the server's pixel and the framebuffer's,
     * with the framebuffer page the pixel is on. */
    char line[128];
    unsigned w, h, depth, pitch = 0;
    int ctl = open("/dev/fb/ctl", O_RDONLY), fd = open("/dev/fb/data", O_RDONLY);
    ssize_t n = ctl < 0 ? -1 : read(ctl, line, sizeof line - 1);
    if (n > 0) {
        line[n] = '\0';
        sscanf(line, "%u %u %u %*s %u", &w, &h, &depth, &pitch);
    }
    for (int y = 0; y < 20; y += 2) {
        XImage *img = XGetImage(dpy, root, X, y, 1, 1, AllPlanes, ZPixmap);
        uint32_t p = 0;
        off_t off = (off_t)y * pitch + X * 4;
        if (fd >= 0 && pitch) {
            lseek(fd, off, SEEK_SET);
            if (read(fd, &p, 4) != 4) {
                p = 0xdeadbeef;
            }
        }
        printf("view: (%d, %d) server %06lx fb %06x page %lld\n", X, y,
               img ? XGetPixel(img, 0, 0) & 0xffffff : 0xffffffUL, (unsigned)(p & 0xffffff), (long long)(off / 4096));
        if (img) {
            XDestroyImage(img);
        }
    }
    XCloseDisplay(dpy);
}

int main(int argc, char **argv) {
    char line[128];
    unsigned w, h, depth, pitch;
    uint32_t p = 0;
    ssize_t n;
    int ctl, fd, want_bar;

    if (argc != 2 || (strcmp(argv[1], "server") && strcmp(argv[1], "bar") && strcmp(argv[1], "gone")
                      && strcmp(argv[1], "probe"))) {
        fprintf(stderr, "usage: dwm_smoke server|bar|gone|probe\n");
        return 2;
    }
    if (!strcmp(argv[1], "probe")) {
        conversations();
        if (probe() != 0) {
            return 1;
        }
        server_view();
        return 0;
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
