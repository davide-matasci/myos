/* st-smoke: the st boot test's eyes on the framebuffer.
 *
 *   st_smoke server  wait (up to 60 s) for the X server's socket, so that
 *                    st (which does not retry) finds it
 *   st_smoke text    wait for text in st's first line: lit pixels in the
 *                    screen's top left corner (st's window, with no window
 *                    manager, sits at 0,0 on the black root)
 *   st_smoke focus   give st's window (the only top-level one) the
 *                    keyboard's focus, as a window manager would: with no
 *                    pointer device, the server's focus does not follow a
 *                    window mapped under the pointer
 *   st_smoke probe   the server's view when the typed keys did not arrive:
 *                    the pointer, the input focus and the top-level windows
 *
 * Prints "[ OK ] st <mode>", or what it last saw.
 */
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>
#include <X11/Xlib.h>

/* The corner looked at: a few characters of the first line. */
#define BOX_W 64
#define BOX_H 20
#define MIN_LIT 30

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

/* The lit pixels in the corner box. */
static int lit_pixels(int fd, unsigned pitch) {
    uint32_t row[BOX_W];
    int lit = 0;
    for (int y = 0; y < BOX_H; y++) {
        if (lseek(fd, (off_t)y * pitch, SEEK_SET) < 0 || read(fd, row, sizeof row) != sizeof row) {
            return -1;
        }
        for (int x = 0; x < BOX_W; x++) {
            lit += (row[x] & 0xffffff) != 0;
        }
    }
    return lit;
}

/* Where the keys go: the pointer (the focus follows it, PointerRoot, with
 * no window manager), the focus and each top-level window. */
static int probe(void) {
    Display *dpy = XOpenDisplay(":0");
    Window root, child, focus, parent, *kids = NULL;
    int rx, ry, wx, wy, revert;
    unsigned mask, nkids = 0;

    if (!dpy) {
        printf("probe: no display\n");
        return 1;
    }
    root = DefaultRootWindow(dpy);
    XQueryPointer(dpy, root, &root, &child, &rx, &ry, &wx, &wy, &mask);
    XGetInputFocus(dpy, &focus, &revert);
    printf("probe: pointer %d,%d over 0x%lx, focus 0x%lx\n", rx, ry, child, focus);
    if (XQueryTree(dpy, root, &root, &parent, &kids, &nkids)) {
        for (unsigned i = 0; i < nkids; i++) {
            XWindowAttributes wa;
            if (XGetWindowAttributes(dpy, kids[i], &wa)) {
                printf("probe: window 0x%lx %dx%d+%d+%d %s\n", kids[i], wa.width, wa.height, wa.x, wa.y,
                       wa.map_state == IsViewable ? "viewable" : "unmapped");
            }
        }
        XFree(kids);
    }
    XCloseDisplay(dpy);
    return 0;
}

/* Focus the first viewable top-level window, waiting (up to 30 s) for st
 * to map it. */
static int focus(void) {
    Display *dpy = XOpenDisplay(":0");
    Window win = None;

    if (!dpy) {
        printf("[ FAIL ] st focus: no display\n");
        return 1;
    }
    for (int tries = 0; tries < 30 && win == None; tries++) {
        Window root, parent, *kids = NULL;
        unsigned nkids = 0;
        if (tries) {
            sleep(1);
        }
        if (XQueryTree(dpy, DefaultRootWindow(dpy), &root, &parent, &kids, &nkids)) {
            for (unsigned i = 0; i < nkids && win == None; i++) {
                XWindowAttributes wa;
                if (XGetWindowAttributes(dpy, kids[i], &wa) && wa.map_state == IsViewable) {
                    win = kids[i];
                }
            }
            XFree(kids);
        }
    }
    if (win == None) {
        printf("[ FAIL ] st focus: no window\n");
        return 1;
    }
    XSetInputFocus(dpy, win, RevertToPointerRoot, CurrentTime);
    XSync(dpy, False);
    XCloseDisplay(dpy);
    printf("[ OK ] st focus\n");
    return 0;
}

int main(int argc, char **argv) {
    char line[128];
    unsigned w, h, depth, pitch;
    int ctl, fd, lit = 0;
    ssize_t n;

    if (argc != 2
        || (strcmp(argv[1], "server") && strcmp(argv[1], "text") && strcmp(argv[1], "focus")
            && strcmp(argv[1], "probe"))) {
        fprintf(stderr, "usage: st_smoke server|text|focus|probe\n");
        return 2;
    }
    if (!strcmp(argv[1], "focus")) {
        return focus();
    }
    if (!strcmp(argv[1], "probe")) {
        return probe();
    }
    if (!strcmp(argv[1], "server")) {
        for (int i = 0; i < 60; i++) {
            if (server_up()) {
                printf("[ OK ] st server\n");
                return 0;
            }
            sleep(1);
        }
        printf("[ FAIL ] st server: no socket\n");
        return 1;
    }
    ctl = open("/dev/fb/ctl", O_RDONLY);
    n = ctl < 0 ? -1 : read(ctl, line, sizeof line - 1);
    if (n <= 0) {
        printf("[ FAIL ] st reading /dev/fb/ctl\n");
        return 1;
    }
    line[n] = '\0';
    close(ctl);
    if (sscanf(line, "%u %u %u %*s %u", &w, &h, &depth, &pitch) != 4 || depth != 32 || w < BOX_W
        || h < BOX_H) {
        printf("[ FAIL ] st /dev/fb/ctl is not a 32-bit screen: %s", line);
        return 1;
    }
    fd = open("/dev/fb/data", O_RDONLY);
    if (fd < 0) {
        printf("[ FAIL ] st opening /dev/fb/data\n");
        return 1;
    }
    for (int i = 0; i < 60; i++) {
        lit = lit_pixels(fd, pitch);
        if (lit < 0) {
            printf("[ FAIL ] st reading /dev/fb/data\n");
            return 1;
        }
        if (lit >= MIN_LIT) {
            printf("[ OK ] st text\n");
            return 0;
        }
        sleep(1);
    }
    printf("[ FAIL ] st text: %d lit pixels in the top left %dx%d\n", lit, BOX_W, BOX_H);
    return 1;
}
